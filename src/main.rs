use humantime::{format_duration, parse_duration};
use k8s_openapi::api::core::v1::{Namespace, Pod};
use kube::{
	api::{Api, DeleteParams, ListParams, ResourceExt},
	Client,
};
use log::{info, log_enabled};
use rand::prelude::*;
use std::{
	cmp::max,
	collections::{HashMap, HashSet},
	error::Error,
	result::Result,
	time::Duration,
};
use structopt::StructOpt;
use tokio::time::sleep;

#[derive(StructOpt)]
enum DeleteMode {
	/// Kill a fixed number of each pod group
	Fixed {
		number_of_pods: usize,
	},
	/// Kill pods until a fixed number of each pod group is alive.
	FixedLeft {
		number_of_pods_left_after_chaos: usize,
	},
	/// Kill a percentage of each pod group (rounded down)
	Percentage {
		percentage_of_pods: usize,
	},
}

#[derive(StructOpt)]
#[structopt(name = "khaos-monkey")]
struct Opt {
	#[structopt(subcommand)]
	mode: DeleteMode,

	/// namespaces you want the monkey to target. Example: "namespace1, namespace2". The monkey will target all pods in these namespace unless they opt-out.
	#[structopt(long, env, default_value = "default")]
	target_namespaces: String,

	/// namespaces you want the monkey to ignore. Pods running in these namespaces can't be target.
	#[structopt(long, env, default_value = "kube-system, kube-public, kube-node-lease")]
	blacklisted_namespaces: String,

	/// Number of pod-types that can be deleted at a time. No limit if value is -1. Example: if set to "2" it may attack two ReplicaSets.
	#[structopt(long, env, default_value = "1")]
	attacks_per_interval: i32,

	/// If "true" a number between 0 and 1 is multiplied with number of pods to kill.
	#[structopt(long, env)]
	random_kill_count: bool,

	/// Minimum time between chaos attacks.
	#[structopt(long, env, default_value = "1m")]
	min_time_between_chaos: String,

	/// This specifies a random time interval that will be added to `min-time-between-chaos` each attack. Example: If both options are set to `1m` the attacks will happen with a random time interval between 1 and 2 minutes.
	#[structopt(long, env, default_value = "1m")]
	random_extra_time_between_chaos: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
	env_logger::init();
	let opt = Opt::from_args();

	let min_time_between_chaos = parse_duration(&opt.min_time_between_chaos).expect("Failed to parse min-time-between-chaos");
	let random_extra_time_between_chaos = parse_duration(&opt.random_extra_time_between_chaos).expect("Failed to parse random-time-between-chaos");

	let mode = opt.mode;
	let random = opt.random_kill_count;

	let mut rng = rand::thread_rng();
	let client = Client::try_default().await?;

	let pod_api: Api<Pod> = Api::all(client.clone());
	let targeted_namespaces: HashSet<String> = get_targeted_namespace(&opt.target_namespaces, &opt.blacklisted_namespaces, &client).await?;

	loop {
		println!("###################");
		println!("### Chaos Beginning\n");

		let grouped_pods = get_grouped_pods(&pod_api, &targeted_namespaces).await?;

		let num_attacks = if opt.attacks_per_interval > -1 {
			opt.attacks_per_interval as usize
		} else {
			grouped_pods.len()
		};

		if grouped_pods.is_empty() {
			println!("Killed no pods");
		} else {

			println!("Attacking {} out of {} pod groups\n", num_attacks, grouped_pods.len());

			for (khaos_group_key, pods) in grouped_pods.iter().take(num_attacks) {
				let pods_to_delete = match mode {
					DeleteMode::Fixed { number_of_pods } => number_of_pods as f32,
					DeleteMode::Percentage { percentage_of_pods } => (pods.len() * percentage_of_pods) as f32 / 100.0,
					DeleteMode::FixedLeft { number_of_pods_left_after_chaos } => max(0, pods.len() - number_of_pods_left_after_chaos) as f32,
				};

				let pods_to_delete = if random {
					pods_to_delete * &rng.gen::<f32>()
				} else {
					pods_to_delete
				} as u32;

				println!("# Deleting: {}/{} running pods in Khaos Group: {}", pods_to_delete, pods.len(), khaos_group_key);

				let mut pods_clone = pods.clone();
				pods_clone.shuffle(&mut rng);
				for pod in pods_clone.iter().take(pods_to_delete as usize) {
					delete_pod(client.clone(), pod).await?;
				}
			}
		}
		println!("\n### Chaos over");

		let wait_time =
			min_time_between_chaos + Duration::from_secs((random_extra_time_between_chaos.as_secs() as f64 * &rng.gen::<f64>()) as u64);

		println!("### Time until next Chaos: {}", format_duration(wait_time));
		println!("###################\n");

		sleep(wait_time).await;
	}
}

async fn get_targeted_namespace(
	target_namespaces: &str,
	blacklisted_namespaces: &str,
	client: &Client,
) -> Result<HashSet<String>, Box<dyn Error>> {
	let namespace_api: Api<Namespace> = Api::all(client.clone());

	let comma_string_to_set =
		|port: &str| port.split(',').into_iter().map(|n| String::from(n.trim())).filter(|n| n != "").collect::<HashSet<String>>();

	let target_namespaces: HashSet<String> = comma_string_to_set(target_namespaces);
	println!("target_namespaces from args/env: {:?}", target_namespaces);

	let blacklisted_namespaces: HashSet<String> = comma_string_to_set(blacklisted_namespaces);
	println!("blacklisted_namespaces from args/env: {:?}", blacklisted_namespaces);

	if !target_namespaces.is_disjoint(&blacklisted_namespaces) {
		panic!("a namespace can't be both in target_namespaces and namespaces_blacklist");
	};

	let namespaces_in_cluster: HashSet<String> = namespace_api.list(&ListParams::default()).await?.iter().map(|n| n.name_any()).collect();
	println!("Namespaces found in cluster: {:?}\n", namespaces_in_cluster);

	let target_namespaces_in_cluster: HashSet<String> =	target_namespaces.intersection(&namespaces_in_cluster).map(|s| String::from(s)).collect();
	println!("Monkey will target namespace: {:?}\n", target_namespaces_in_cluster);
	Ok(target_namespaces_in_cluster)
}

async fn get_grouped_pods(pods: &Api<Pod>, targeted_namespaces: &HashSet<String>) -> Result<HashMap<String, Vec<Pod>>, Box<dyn Error>> {
	let mut map: HashMap<String, Vec<Pod>> = HashMap::new();
	info!("## All pods found:");
	for pod in pods.list(&ListParams::default()).await? {
		let in_targeted_namespace = targeted_namespaces.contains(&pod.namespace().unwrap_or_default());
		let labels = pod.labels();

		info!("- {}", pod.name_any());

		match (labels.get("khaos-enabled"), in_targeted_namespace) {
			(None, false) => continue,
			(Some(khaos), false) if khaos != "true" => continue,
			(Some(khaos), _) if khaos == "false" => continue,
			_ => (),
		};

		let khaos_group = labels
			.get("khaos-group")
			.map(|l| String::from(l))
			.or(labels.iter().find(|l| l.0.contains("pod-template-hash")).map(|l| format!("{}={}", *l.0, *l.1)))
			.or(labels.iter().find(|l| l.0 == "job-name").map(|l| format!("{}={}", *l.0, *l.1)));

		if let Some(group) = khaos_group {
			match map.get_mut(&group) {
				Some(v) => v.insert(0, pod),
				None => {
					map.insert(group.to_string(), vec![pod]);
				}
			}
		};
	}

	info!("## \n");

	if log_enabled!(log::Level::Info) {
		info!("## All targeted groups:");
		for (group_name, pods) in &map {
			info!("- {} with {} pods:", group_name, pods.len());
			for pod in pods {
				info!("  - {}", pod.name_any());
			}
		}
		info!("## \n");
	}

	Ok(map)
}

async fn delete_pod(client: Client, pod: &Pod) -> Result<(), Box<dyn Error>> {
	let api: Api<Pod> = Api::namespaced(client, &pod.namespace().unwrap());
	api.delete(&pod.name_any(), &DeleteParams::default())
		.await?
		.map_left(|o| println!("Deleting Pod: {:?}", o.name_any()))
		.map_right(|s| println!("Deleted Pod: {:?}", s));
	Ok(())
}

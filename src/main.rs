use humantime::{format_duration, parse_duration};
use k8s_openapi::api::core::v1::{Namespace, Pod};
use structopt::StructOpt;
use tokio::time::sleep;
use rand::prelude::*;
use kube::{Client, api::{Api, DeleteParams, ListParams, ResourceExt}};
use std::{
	cmp::max,
	collections::{HashMap, HashSet},
	error::Error,
	result::Result,
	str::FromStr,
	time::Duration,
};

enum DeleteMode {
	Fixed,
	FixedLeft,
	Percentage,
}

impl FromStr for DeleteMode {
	type Err = ();

	fn from_str(input: &str) -> Result<DeleteMode, Self::Err> {
		match input {
			"fixed" => Ok(DeleteMode::Fixed),
			"fixed_left" => Ok(DeleteMode::FixedLeft),
			"percentage" => Ok(DeleteMode::Percentage),
			_ => Err(()),
		}
	}
}

#[derive(Debug, StructOpt)]
#[structopt(name = "khaos-monkey")]
struct Opt {
	/// Can be `fixed`, `fixed_left`, or `percentage`. If set to `percentage` they monkey will kill a given percentage of targeted pods. If set to `fixed` they will kill a fixed number (`value`) of pods each type. If set to `fixed_left` they will kill all pod types until there is `value` pods left.
	#[structopt(long, env, default_value = "fixed")]
	mode: String,

	/// The number of pods to kill each type. The 
	#[structopt(long, env, default_value = "1")]
	kill_value: usize,
	
	/// namespaces you want the monkey to target. Example: "namespace1, namespace2". The monkey will target all pods in these namespace unless they opt-out.
	#[structopt(long, env, default_value = "default")]
	target_namespaces: String,
	
	/// namespaces you want the monkey to ignore. Pods that opt-in running in these namespaces will also be ignored.
	#[structopt(long, env, default_value = "kube-system, kube-public, kube-node-lease")]
	blacklisted_namespaces: String,

	/// Number of pod-types that can be deleted at a time. No limit if value is -1. Example: if set to "2" it may attack two replicasets. 
	#[structopt(long, env, default_value = "1")]
	attacks_per_interval: i32,

	/// If "true" a number between 0 and 1 is multiplied with number of pods to kill. 
	#[structopt(long, env)]
	random_kill_count: bool,

	/// Minimum time between chaos attacks.
	#[structopt(long, env, default_value = "1m")]
	min_time_between_chaos: String,

	/// This specifies a random time interval that will be added to `min_time_between_chaos` each attack. Example: If both options are sat to `1m` the attacks will happen with a random time interval between 1 and 2 minutes.
	#[structopt(long, env, default_value = "1m")]
	random_extra_time_between_chaos: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
	std::env::set_var("RUST_LOG", "info,kube=debug");

	start().await?;
	Ok(())
}

async fn start() -> Result<(), Box<dyn Error>> {
	println!("Starting");

	let opt = Opt::from_args();

	let min_time_between_chaos = parse_duration(&opt.min_time_between_chaos).expect("Failed to parse min_time_between_chaos");
	let random_extra_time_between_chaos = parse_duration(&opt.random_extra_time_between_chaos).expect("Failed to parse random_time_between_chaos");

	let mode = DeleteMode::from_str(&opt.mode).expect("`mode` not valid. Run with `--help` for more info.");
	let random = opt.random_kill_count;
	let value = opt.kill_value;
	let num_attacks = if opt.attacks_per_interval > -1 {
		opt.attacks_per_interval
	} else {
		10000
	};

	let mut rng = rand::thread_rng();
	let client = Client::try_default().await?;

	let pod_api: Api<Pod> = Api::all(client.clone());
	let targeted_namespace: HashSet<String> = get_targeted_namespace(opt, &client).await?;

	loop {
		println!("###################");
		println!("### Chaos Beginning");
		for (khaos_key, pods) in get_grouped_pods(&pod_api, &targeted_namespace).await?.iter().take(num_attacks as usize) {
			let pods_to_delete = match mode {
				DeleteMode::Fixed => value as f32,
				DeleteMode::Percentage => (pods.len() * value) as f32 / 100.0,
				DeleteMode::FixedLeft => max(0, pods.len() - value) as f32,
			};
			let pods_to_delete = if random {
				pods_to_delete * &rng.gen::<f32>()
			} else {
				pods_to_delete
			} as usize;

			println!("# Khaos Group: {} - Count: {} - Deleting: {}", khaos_key, pods.len(), pods_to_delete as u32);

			let mut pods_clone = pods.clone();
			pods_clone.shuffle(&mut rng);
			for pod in pods_clone.iter().take(pods_to_delete as usize) {
				delete_pod(client.clone(), pod).await?;
			}
		}
		println!("");
		println!("### Chaos over");

		let wait_time = min_time_between_chaos + Duration::from_secs((random_extra_time_between_chaos.as_secs() as f64 * &rng.gen::<f64>()) as u64);
		
			println!("Time until next Chaos: {}", format_duration(wait_time));
		println!("###################");
		println!("");

		sleep(wait_time).await;
	}
}

async fn get_targeted_namespace(opt: Opt, client: &Client) -> Result<HashSet<String>, Box<dyn Error>> {

	let namespace_api: Api<Namespace> = Api::all(client.clone());

	let comma_string_to_set = |port: String| port.split(',').into_iter().map(|n| String::from(n.trim())).filter(|n| n != "").collect::<HashSet<String>>();

	let target_namespaces: HashSet<String> = comma_string_to_set(opt.target_namespaces);
	println!("target_namespaces: {:?}", target_namespaces);
	
	let namespaces_blacklist: HashSet<String> =	comma_string_to_set(opt.blacklisted_namespaces);
	println!("blacklisted: {:?}", namespaces_blacklist);
	
	if !target_namespaces.is_disjoint(&namespaces_blacklist) {
		panic!("a namespace can't be both in target_namespaces and namespaces_blacklist");
	};

	let namespaces_in_cluster: HashSet<String> = namespace_api.list(&ListParams::default()).await?.iter().map(|n| n.name()).collect();
	println!("Namespaces found in cluster: {:?}", namespaces_in_cluster);

	let accepted_namespaces: HashSet<String> = target_namespaces.intersection(&namespaces_in_cluster).map(|s| String::from(s)).collect();
	println!("Monkey will target: {:?}", accepted_namespaces);
	Ok(accepted_namespaces)
}

async fn get_grouped_pods(pods: &Api<Pod>, targeted_namespace: &HashSet<String>) -> Result<HashMap<String, Vec<Pod>>, Box<dyn Error>> {
	let mut map: HashMap<String, Vec<Pod>> = HashMap::new();
	for pod in pods.list(&ListParams::default()).await? {
		let in_targeted_namespace = targeted_namespace.contains(&pod.namespace().unwrap_or_default());
		let labels = pod.labels();
		
		match (labels.get("khaos-enabled"), in_targeted_namespace) {
			(None, false) => continue,
			(Some(khaos), false) if khaos != "true" => continue,
			(Some(khaos), _) if khaos == "false" => continue,
			_ => (),
		};

		let khaos_group = labels
			.get("khaos-group")
			.map(|l| String::from(l))
			.or(labels.iter().find(|l| l.0.contains("pod-template-hash")).map(|l| format!("{}={}", *l.0, *l.1)));
		
		if let Some(group) = khaos_group {
			match map.get_mut(&group) {
				Some(v) => v.insert(0, pod),
				None => {
					map.insert(group.to_string(), vec![pod]);
				}
			}
		};
	}

	Ok(map)
}

async fn delete_pod(client: Client, pod: &Pod) -> Result<(), Box<dyn Error>> {
	let api: Api<Pod> = Api::namespaced(client, &pod.namespace().unwrap());
	api.delete(&pod.name(), &DeleteParams::default())
		.await?
		.map_left(|o| println!("Deleting Pod: {:?}", o.name()))
		.map_right(|s| println!("Deleted Pod: {:?}", s));
	Ok(())
}

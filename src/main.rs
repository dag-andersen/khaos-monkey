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
	/// Namespace
	#[structopt(long, env, default_value = "default")]
	namespace: String,

	#[structopt(long, env, default_value = "1")]
	value: usize,

	/// Can be fixed, fixed_left, or percentage.
	#[structopt(long, env, default_value = "fixed")]
	mode: String,

	/// Number of types that can be deleted at a time. no limit if value is -1.
	#[structopt(long, env, default_value = "1")]
	attacks_per_interval: i32,

	/// If true a number between 0 and 1 is multiplied with number of pods to kill.
	#[structopt(long, env)]
	random: bool,

	/// If true a number between 0 and 1 is multiplied with number of pods to kill.
	#[structopt(long, env, default_value = "default")]
	white_namespaces: String,

	/// If true a number between 0 and 1 is multiplied with number of pods to kill.
	#[structopt(long, env, default_value = "kube-system, kube-public, kube-node-lease")]
	black_namespaces: String,

	/// Minimum time between chaos attacks.
	#[structopt(long, env, default_value = "1m")]
	min_time_between_chaos: String,

	/// This specifies how often the chaos attack happens.
	#[structopt(long, env, default_value = "1m")]
	random_time_between_chaos: String,

	/// This specifies how often the chaos attack happens.
	#[structopt(long, env, default_value = "")]
	blacklisted_namespace: String,
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

	parse_duration(&opt.min_time_between_chaos).expect("Failed to parse min_time_between_chaos");
	parse_duration(&opt.random_time_between_chaos).expect("Failed to parse random_time_between_chaos");

	let mode = DeleteMode::from_str(&opt.mode).expect("msg");
	let random = opt.random;
	let value = opt.value;
	let num_attacks = if opt.attacks_per_interval > -1 {
		opt.attacks_per_interval
	} else {
		10000
	};

	let mut rng = rand::thread_rng();
	let client = Client::try_default().await?;

	let pod_api: Api<Pod> = Api::all(client.clone());
	let aa: Api<Namespace> = Api::all(client.clone());

	let namespaces_whitelist: HashSet<String> =
		opt.white_namespaces.split(',').into_iter().map(|n| String::from(n.trim())).filter(|n| n != "").collect();
	println!("whitelisted: {:?}", namespaces_whitelist);
	let namespaces_blacklist: HashSet<String> =
		opt.black_namespaces.split(',').into_iter().map(|n| String::from(n.trim())).filter(|n| n != "").collect();
	println!("blacklisted: {:?}", namespaces_blacklist);
	if !namespaces_whitelist.is_disjoint(&namespaces_blacklist) {
		println!("a namespace can't be both in whitelist and blacklist");
		return Ok(());
	};
	let namespaces_in_cluster: HashSet<String> = aa.list(&ListParams::default()).await?.iter().map(|n| n.name()).collect();
	println!("Namespaces found on cluster: {:?}", namespaces_in_cluster);

	let accepted_namespaces: HashSet<String> = namespaces_whitelist.intersection(&namespaces_in_cluster).map(|s| String::from(s)).collect();
	println!("Accepted Namespaces: {:?}", accepted_namespaces);

	loop {
		println!("###################");
		println!("### Chaos Beginning");
		for (khaos_key, pods) in get_grouped_pods(&pod_api, &accepted_namespaces).await?.iter().take(num_attacks as usize) {
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

		let wait_time = parse_duration(&opt.min_time_between_chaos)?
			+ Duration::from_secs((parse_duration(&opt.random_time_between_chaos)?.as_secs() as f64 * &rng.gen::<f64>()) as u64);
		println!("Time until next Chaos: {}", format_duration(wait_time));
		println!("###################");
		println!("");

		sleep(wait_time).await;
	}
}

async fn get_grouped_pods(pods: &Api<Pod>, allowed_namespaces: &HashSet<String>) -> Result<HashMap<String, Vec<Pod>>, Box<dyn Error>> {
	let mut map: HashMap<String, Vec<Pod>> = HashMap::new();
	for p in pods.list(&ListParams::default()).await? {
		let in_namespace = allowed_namespaces.contains(&p.namespace().unwrap_or_default());
		let labels = p.labels();
		match (labels.get("khaos-enabled"), in_namespace) {
			(None, false) => continue,
			(Some(khaos), false) if khaos != "true" => continue,
			(Some(khaos), _) if khaos == "false" => continue,
			_ => (),
		};

		let khaos_group = labels
			.get("khaos-group")
			.map(|x| String::from(x))
			.or(labels.iter().find(|x| x.0.contains("pod-template-hash")).map(|x| format!("{}={}", *x.0, *x.1)));

		if let Some(kg) = khaos_group {
			match map.get_mut(&kg) {
				Some(v) => {
					v.insert(0, p);
				}
				None => {
					map.insert(kg.to_string(), vec![p]);
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

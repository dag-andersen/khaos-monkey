![](./images/khaos-monkey-01.png)

# khaos-monkey - A simple Chaos Monkey for Kubernetes
Built on top of [kube-rs](https://github.com/kube-rs/kube-rs)


# Why would you need this monkey?

Read about Chaos Engineering

# Features

## Target Grouping

By default all pods in a replicaset is grouped. So if a Deployment has 4 replicas the monkey may kill 1 of those replicas. 
You can make a custom group by adding an annothatuion to the pods - e.g. `khaos-group=my-group`. This would make the make the monkey treat your custom group the way it treats a replicaset. 

## Targeting based on namespaces

The monkey can either target individual pods or whole namespaces. If the option `target_namespaces` is sat to `namespaceA, namespaceB` the monkey will target all pods in those two namespaces. This means that the monkey may kill any pods in those namespaces.

> Running 

## Opt-in

This feature means you can make the monkey target individual pod in any namespace by adding the label `khaos-enabled: true` to it. If this label exist on a pod it doesn't matter if it inside in the namespaces specified by `target_namespaces`. 

*Opt-in deployment example*:
```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: whatever-deployment
spec:
  template:
    spec:
      metadata:
        labels:
          khaos-enabled: "true"
      containers:
      - name: echo-client
        image: whatever-image
```

*Opt-in cronjob example*:
```yaml
apiVersion: apps/v1
kind: Cronjob  something 
metadata:
  name: whatever-deployment
spec:
  template:
    spec:
... . . . . ..  . . ..  . ... ..  ... .
```

## Opt-out

This feature means you can enable the monkey to target on all pods by default on specific namespaces and choose which pods you want to be excluded in those namespaces. All pods with the label `khaos-enabled: false` will opt-out and be excluded in the pod targeting by the monkey.

> Example: The monkey is targeting `namespaceA` and `podA` exist inside that namespace. If the pod has the annotation `khaos-enabled: false` it will not be ignored by the monkey and not killed - if it does not have that annotation it will be targeted by the monkey (and eventually be killed). 

*Opt-out Example*:
```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: whatever-deployment
spec:
  template:
    spec:
    metadata:
        labels:
          khaos-enabled: "false"
      containers:
      - name: echo-client
        image: whatever-image
```

## 3 Modes

The the usage of the option `kill_value` all depends on what mode the monkey is using.

* `percentage` - The monkey will kill a given percentage of targeted pods. (The number is rounded down). Example: if you set the `kill_value` to `55` and your replicaset has 4 pods the monkey will kill 2 random pods on every attack.
* `fixed` - If sat to `fixed` they will kill a fixed number (`kill_value`) of pods each type. Example: if you set the `kill_value` to `3` and your replicaset has 5 pods the monkey will kill 3 random pods on every attack.
* `fixed_left` - If sat to `fixed_left` they will kill all pod types until there is `kill_value` pods left. Example: if you set the `kill_value` to `3` and your replicaset has 5 pods the monkey will pods until there is 3 left. In this case it would kill 2 pods.

## Randomness

You can randomize how often the attack happens and how many pods are killed each attack.

You can set `random-kill-count` to `true` if you want the monkey to kill a random amount of pods between 0 and the `kill-value`.
> Example: If the monkey run with `--mode=percentage --kill-value=50 --random-kill-count=true` then the monkey will kill between 0 and 50 percent of the pods in each replacaset.

You can set `random-extra-time-between-chaos` to `5m` if you want you want to add additional random time between each attack.
> Example: If the monkey run with `--min-time-between-chaos=1m --random-extra-time-between-chaos=1m` the attacks will happen with a random time interval between 1 and 2 minutes.

## Default Settings (Dont worry - it wont kill anything before you instruct it to)

By default it does not target any namespaces, so it wont start killing pods until you specify namespaces to target or you make pods opt-in. 

# Installation

### Create the namespace:
```bash
$ kubectl create namespace khaos-monkey
```
### Create the right permission with rbac:

Either by referering to the repo file:
```bash
$ kubectl apply 
```
or run:
```yaml
cat <<EOF | kubectl apply -f -
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  namespace: default
  name: khaos-monkey-cluster-role
rules:
- apiGroups: ["*"]
  resources: ["pods", "namespaces"]
  verbs: ["list", "delete"]
---
kind: ClusterRoleBinding
apiVersion: rbac.authorization.k8s.io/v1
metadata:
  name: khaos-monkey-cluster-role-binding
subjects:
- kind: ServiceAccount
  name: default
  namespace: default
roleRef:
  kind: ClusterRole
  name: khaos-monkey-cluster-role
  apiGroup: ""
```

### Deploy the monkey.

> Feel free to tune the numbers yourself. Remember that the monkey may kill it self if it exist inside a targeted namespace and does not not [opt-out](#Opt-out). It is possible to run multiple instances of the monkey with different settings. 

Either by referering to the repo file:
```bash
$ kubectl apply -f  
```
or run:

```yaml
cat <<EOF | kubectl apply -f -
apiVersion: apps/v1
kind: Deployment
metadata:
  name: khaos-monkey-deployment
spec:
  minReadySeconds: 15
  replicas: 1
  strategy:
    type: Recreate
  template:
    metadata:
      labels:
        khaos-enabled: "false"
    spec:
      containers:
      - name: khaos-monkey
        image: dagandersen/khaos-monkey:latest
        resources:
          requests:
            cpu: "0"
            memory: "0"
          limits:
            cpu: "250m"
            memory: "1Gi"
        env:
          - name: attacks-per-interval
            value: "1"
EOF
```

# All CLI Options

```bash
khaos-monkey 0.1.0

USAGE:
    khaos-monkey [OPTIONS]

OPTIONS:
    --mode <mode> [default: fixed]
        Can be fixed, fixed_left, or percentage

    --kill_value [default: 1]

    --target_namespaces [default: default]
        Namespace [default: default]
    
    --blacklisted-namespace [default: kube-system, kube-public, kube-node-lease]
        This specifies how often the chaos attack happens

    --attacks-per-interval [default: 1]
        Number of types that can be deleted at a time. no limit if value is -1

    --random-kill-count [default: false]
        If true a number between 0 and 1 is multiplied with number of pods to kill

    --min-time-between-chaos [default: 1m]
        Minimum time between chaos attacks

    --random-extra-time-between-chaos [default: 1m]
        This specifies how often the chaos attack happens [default: 1m]
```

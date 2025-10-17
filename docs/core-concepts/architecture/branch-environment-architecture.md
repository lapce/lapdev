# Branch Environment Architecture

Branch environments are a cost-effective way to run development environments in Kubernetes. Instead of duplicating all services for each developer, branch environments share a baseline and only run the services you're actively modifying.

### Architecture Diagram

<img src="../../.gitbook/assets/file.excalidraw (3).svg" alt="" class="gitbook-drawing">

### Concept

Without Branch Environments:

```
Alice's environment: 100 services (all running separately)
Bob's environment: 100 services (all running separately)
Total: 200 services
```

With Branch Environments:

```
Baseline environment: 100 services (shared by everyone)
Alice's branch: 1 service (api - her modified version)
Bob's branch: 1 service (worker - his modified version)
Total: 102 services
```

### How It Works

#### The Baseline Environment

One shared environment runs all your services:

* Contains a complete copy of all workloads
* Shared by all developers
* Updated when production manifests change

#### Your Branch Environment

When you create a branch, you specify which service(s) you're modifying:

* Only those services run in your branch namespace
* Everything else uses the baseline

#### Traffic Routing

When someone accesses your preview URL, Lapdev routes traffic intelligently:

Example: Alice's branch modifies the `api` service

Request to `https://alice-branch.app.lap.dev`:

1. Starts at baseline environment
2. When it needs to call the `api` service → routes to Alice's branch
3. When it needs `web`, `worker`, or any other service → uses baseline

Example: Bob's branch modifies the `worker` service

Request to `https://bob-branch.app.lap.dev`:

1. Starts at baseline environment
2. When it needs to call the `worker` service → routes to Bob's branch
3. When it needs `api`, `web`, or any other service → uses baseline

#### How Routing Works

Each request gets a special identifier (tracestate) that tells services which branch it belongs to. The Lapdev Sidecar Proxy in each pod:

1. Checks if the service being called has a branch override
2. Routes to the branch if it exists
3. Routes to baseline if no override exists
4. Passes the identifier along to the next service

This happens automatically - your code doesn't need any changes.

### Visual Example

```
Baseline Environment:
├── api (baseline version)
├── web (baseline version)
└── worker (baseline version)

Alice's Branch:
└── api (Alice's custom version)

Bob's Branch:
└── worker (Bob's custom version)
```

Traffic to alice-branch.app.lap.dev:

* api → Alice's version ✓
* web → baseline version
* worker → baseline version

Traffic to bob-branch.app.lap.dev:

* api → baseline version
* web → baseline version
* worker → Bob's version ✓

Traffic to baseline.app.lap.dev:

* api → baseline version
* web → baseline version
* worker → baseline version

### Key Benefits

* Cost-effective: Only run what you're changing (10x cheaper than isolated environments)
* Instant creation: Branch environments are created instantly - custom workloads are only deployed when you modify it.

### Limitations

* Shared databases: All branches use the baseline's database (be careful with schema changes)
* Tracestate propagation: Your application needs to pass the tracestate header in service-to-service calls for routing to work correctly

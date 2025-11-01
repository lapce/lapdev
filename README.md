<div align="center">
  <img width=120 height=120 src="https://github.com/lapce/lapdev/assets/164527084/e8ca611c-6288-4ceb-abdd-55f50b43f2a3"></img>

  # Lapdev
  
  **Seamless dev environments for your Kubernetes apps**
</div>

<div align="center">
  <a href="https://github.com/lapce/lapdev/actions/workflows/ci.yml" target="_blank">
    <img src="https://github.com/lapce/lapdev/actions/workflows/ci.yml/badge.svg" />
  </a>
  <a href="https://discord.gg/DTZNfz3Ung" target="_blank">
    <img src="https://img.shields.io/discord/946858761413328946?logo=discord" />
  </a>
  <a href="https://lapdev.gitbook.io/docs/" target="_blank">
      <img src="https://img.shields.io/static/v1?label=Docs&message=https://lapdev.gitbook.io/docs/&color=blue" alt="Lapdev Docs">
  </a>
</div>
<br>

**Lapdev** gives your team production-accurate development environments that run directly in your Kubernetes cluster. Install the lightweight `lapdev-kube-manager`, choose the workloads you care about, and Lapdev keeps isolated or shared environments in sync with your production manifests. Developers iterate quickly using the [Devbox CLI](https://docs.lap.dev/core-concepts/devbox) for local debugging while staying connected to real cluster resources.

<br>

![](https://lap.dev/images/screenshot.png) 

<br>

## What Lapdev Does

- **Reads production manifests** – Lapdev builds an App Catalog straight from your production Deployments, StatefulSets, ConfigMaps, Secrets, and Services.
- **Creates tailored environments** – Launch fully isolated personal namespaces, shared team baselines, or lightweight branch environments with intelligent traffic routing.
- **Keeps everything in sync** – Monitor production for changes, notify developers, and apply updates when you are ready—no manual YAML drift.
- **Enables local-style iteration** – Devbox routes cluster traffic to your laptop, so you debug locally while remaining wired into in-cluster dependencies.
- **Publishes secure preview URLs** – Share HTTPS endpoints for any service without juggling DNS, certificates, or ingress rules.

## Architecture at a Glance

Lapdev is made of three components that work together:

1. **Lapdev API Server** – SaaS control plane that handles auth, orchestration, and secure tunneling.
2. **Lapdev-Kube-Manager** – In-cluster operator that mirrors production workloads into development namespaces.
3. **Devbox CLI** – Developer tooling for traffic interception and access to cluster services from your local machine.

Dig deeper in the [Architecture docs](https://docs.lap.dev/core-concepts/architecture/).

## Getting Started

1. [Connect your Kubernetes cluster](https://docs.lap.dev/how-to-guides/connect-your-kubernetes-cluster)
2. [Create an App Catalog](https://docs.lap.dev/how-to-guides/create-an-app-catalog)
3. [Provision your first environment](https://docs.lap.dev/how-to-guides/create-lapdev-environment)
4. [Develop locally with Devbox](https://docs.lap.dev/how-to-guides/local-development-with-devbox)

## Resources

- [Core concepts](https://docs.lap.dev/core-concepts/README)
- [Traffic routing architecture](https://docs.lap.dev/core-concepts/architecture/traffic-routing-architecture)
- [Preview URLs](https://docs.lap.dev/core-concepts/preview-url)
- [Lapdev Docs](https://docs.lap.dev)

<div align="center">
  <img width=120 height=120 src="https://github.com/lapce/lapdev/assets/164527084/e8ca611c-6288-4ceb-abdd-55f50b43f2a3"></img>

  # Lapdev
  
  **Self-hosted remote development enviroment management with ease**
</div>

<div align="center">
  <a href="https://github.com/lapce/lapdev/actions/workflows/ci.yml" target="_blank">
    <img src="https://github.com/lapce/lapdev/actions/workflows/ci.yml/badge.svg" />
  </a>
  <a href="https://discord.gg/DTZNfz3Ung" target="_blank">
    <img src="https://img.shields.io/discord/946858761413328946?logo=discord" />
  </a>
  <a href="https://docs.lap.dev" target="_blank">
      <img src="https://img.shields.io/static/v1?label=Docs&message=docs.lap.dev&color=blue" alt="Lapdev Docs">
  </a>
</div>
<br>

**Lapdev** is a self hosted application that spins up remote development environments on your own servers or clouds. It scales from a single machine in the corner to a global fleet of servers. It uses [Devcontainer open specification](https://containers.dev/) for defining your development environment as code. If you’re interested in a deep dive into how Lapdev works, you can read about its [architecture](https://docs.lap.dev/administration/architecture) here.

## Demo

You can have a quick feel of how Lapdev works by going to our demo installation https://ws.lap.dev/

We don't request `read_repo` scope for Github Oauth, so you can only play with public repositories. The machine is hosted on Hetzner Germany so there could be some latency if you live far away. 

<br>

![](https://lap.dev/images/screenshot.png) 

<br>

## Features

- **Self hosted with ease:** Lapdev is designed to be self hosted with minimum efforts for installation and maintenance. The application is designed to just work, sparing you from digging too deep into the internals for troubleshooting. 

- **Horizontal scalability:** With a simple yet powerful [architecture](https://docs.lap.dev/administration/architecture), Lapdev can scale from a single machine to a fleet of servers, so that you can have a development environment management system that can grow with your developer teams.

- **Development Environment as Code:** Using the [Devcontainer open specification](https://containers.dev/), Lapdev allows you to define your development environment as code. This empowers you to standardize development environments that can be replicated across different developers, avoiding environment related issues and ensuring a consistent setup for everyone.

- **Save Onboarding Time:** Onboarding developers to new projects don't need hours or days to prepare the environment on their machines. They can start to code instantly.

## Planned Features

- **More workspace types:** Currently Lapdev only supports container based workspaces, which has its own limitations for example when you want to run a k8s cluster in your development flow. It's planned to have support for more than containers. VMs and bare metal machine support are on the roadmap. And more OS support is planned as well, e.g. when you are developing a cross platform desktop application for Windows, Linux and macOS, Lapdev can spin up development environments on all of them and you can develop and debug from the same local machine without the need to switch machines.  

## Installation

You can see the installation steps [here](https://docs.lap.dev/installation/quickstart).

## Build from source

## Contributing

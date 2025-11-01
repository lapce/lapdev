use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, FixedOffset};
use lapdev_api_hrpc::HrpcServiceClient;
use lapdev_common::kube::{
    KubeEnvironment, KubeEnvironmentDashboardSummary, KubeEnvironmentStatus,
    KubeEnvironmentStatusCount,
};
use leptos::prelude::*;

use crate::{
    app::AppConfig,
    component::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonSize, ButtonVariant},
        card::{Card, CardContent, CardDescription, CardHeader, CardTitle},
        table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow},
        typography::{Lead, H2, P},
    },
    organization::get_current_org,
};

#[component]
pub fn DashboardHome() -> impl IntoView {
    let org = get_current_org();
    let hrpc_client = use_context::<Arc<HrpcServiceClient>>().expect("hrpc client missing");

    let summary_resource = LocalResource::new(move || {
        let client = hrpc_client.clone();
        let org = org.get();
        async move {
            let Some(org) = org else {
                return None;
            };
            match client
                .get_environment_dashboard_summary(org.id, Some(6))
                .await
            {
                Ok(Ok(summary)) => Some(summary),
                Ok(Err(err)) => {
                    leptos::logging::error!("failed to load k8s dashboard summary: {err:?}");
                    Some(KubeEnvironmentDashboardSummary::default())
                }
                Err(err) => {
                    leptos::logging::error!("failed to reach dashboard summary endpoint: {err:?}");
                    Some(KubeEnvironmentDashboardSummary::default())
                }
            }
        }
    });

    let summary = Signal::derive(move || summary_resource.get().flatten());
    let is_loading = Signal::derive(move || summary_resource.get().is_none());

    view! {
        <div class="flex flex-col gap-8">
            <section class="flex flex-col gap-4">
                <H2 class="text-3xl font-semibold">"Kubernetes Dev Environments"</H2>
                <Lead class="text-muted-foreground max-w-3xl">
                    "Track the health of your personal, branch, and shared environments. Branch environments borrow every workload from the shared baseline until you override a service, letting you ship feature slices without paying for full clones."
                </Lead>
                <div class="flex flex-wrap gap-3">
                    <a href="/kubernetes/environments/personal" class="inline-flex">
                        <Button size=ButtonSize::Sm variant=ButtonVariant::Outline>
                            <lucide_leptos::User attr:class="h-4 w-4" />
                            "Personal environments"
                        </Button>
                    </a>
                    <a href="/kubernetes/environments/branch" class="inline-flex">
                        <Button size=ButtonSize::Sm variant=ButtonVariant::Outline>
                            <lucide_leptos::GitBranch attr:class="h-4 w-4" />
                            "Branch environments"
                        </Button>
                    </a>
                    <a href="/kubernetes/environments/shared" class="inline-flex">
                        <Button size=ButtonSize::Sm variant=ButtonVariant::Outline>
                            <lucide_leptos::Users attr:class="h-4 w-4" />
                            "Shared environments"
                        </Button>
                    </a>
                    <a href="/kubernetes/catalogs" class="inline-flex">
                        <Button size=ButtonSize::Sm variant=ButtonVariant::Outline>
                            <lucide_leptos::BookOpen attr:class="h-4 w-4" />
                            "Browse app catalogs"
                        </Button>
                    </a>
                </div>
            </section>

            <Show
                when=move || !is_loading.get()
                fallback=move || view! { <DashboardSkeleton /> }
            >
                {move || {
                    let summary = summary.get().unwrap_or_default();
                    view! {
                        <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
                            <MetricCard title="Total environments" value=summary.total_count description="Across personal, branch, and shared spaces." />
                            <MetricCard title="Personal" value=summary.personal_count description="Fully isolated namespaces dedicated to you." />
                            <MetricCard title="Branch" value=summary.branch_count description="Branch copies that inherit shared workloads until you modify them." />
                            <MetricCard title="Shared" value=summary.shared_count description="Team baselines that power integration tests and branch forks." />
                        </div>

                        <div class="grid gap-6 lg:grid-cols-3">
                            <Card class="lg:col-span-2">
                                <CardHeader>
                                    <CardTitle>"Status overview"</CardTitle>
                                    <CardDescription>
                                        "Live status for every environment you can access."
                                    </CardDescription>
                                </CardHeader>
                                <CardContent class="space-y-3">
                                    {render_status_breakdown(&summary.status_breakdown)}
                                </CardContent>
                            </Card>

                            <Card>
                                <CardHeader>
                                    <CardTitle>"Resource tips"</CardTitle>
                                    <CardDescription>
                                        "Keep environments lean to reduce cluster load."
                                    </CardDescription>
                                </CardHeader>
                                <CardContent class="space-y-3 text-sm text-muted-foreground">
                                    <div class="flex items-start gap-2">
                                        <lucide_leptos::Clock attr:class="h-4 w-4 text-primary mt-0.5" />
                                        <span>"Pause idle environments when you wrap up for the day to release compute."</span>
                                    </div>
                                    <div class="flex items-start gap-2">
                                        <lucide_leptos::RefreshCw attr:class="h-4 w-4 text-primary mt-0.5" />
                                        <span>"Sync shared baselines with the latest app catalog to pick up dependency and security fixes."</span>
                                    </div>
                                    <div class="flex items-start gap-2">
                                        <lucide_leptos::LifeBuoy attr:class="h-4 w-4 text-primary mt-0.5" />
                                        <span>"Need a new template? Coordinate with your platform team to publish catalog updates before cutting new branches."</span>
                                    </div>
                                </CardContent>
                            </Card>
                        </div>

                        <Card>
                            <CardHeader>
                                <CardTitle>"Recent environments"</CardTitle>
                                <CardDescription>
                                    "Latest activity across personal, branch, and shared namespaces."
                                </CardDescription>
                            </CardHeader>
                            <CardContent>
                                {render_recent_environments(summary.recent_environments.clone())}
                            </CardContent>
                        </Card>
                    }
                }}
            </Show>
        </div>
    }
}

#[component]
fn MetricCard(title: &'static str, value: usize, description: &'static str) -> impl IntoView {
    view! {
        <Card>
            <CardHeader class="space-y-1">
                <CardTitle class="text-base font-medium text-muted-foreground">{title}</CardTitle>
                <span class="text-3xl font-semibold text-foreground">{value}</span>
            </CardHeader>
            <CardContent class="text-sm text-muted-foreground">{description}</CardContent>
        </Card>
    }
}

#[component]
fn DashboardSkeleton() -> impl IntoView {
    view! {
        <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
            {(0..4).map(|_| {
                view! {
                    <div class="h-36 animate-pulse rounded-lg border bg-white/60" />
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}

fn render_status_breakdown(statuses: &[KubeEnvironmentStatusCount]) -> AnyView {
    let mut counts_by_label: BTreeMap<&'static str, usize> = BTreeMap::new();
    for status in statuses {
        let label = status_label(&status.status);
        *counts_by_label.entry(label).or_default() += status.count;
    }

    if counts_by_label.is_empty() {
        return view! {
            <P class="text-sm text-muted-foreground">
                "No environments yet. Launch one from the app catalog to get started."
            </P>
        }
        .into_any();
    }

    let mut totals: Vec<(String, usize)> = counts_by_label
        .into_iter()
        .map(|(label, count)| (label.to_string(), count))
        .collect();
    totals.sort_by(|a, b| b.1.cmp(&a.1));

    view! {
        {totals
            .into_iter()
            .map(|(label, count)| {
                view! {
                    <div class="flex items-center justify-between rounded-md border border-border/60 px-3 py-2">
                        <span class="text-sm font-medium">{label}</span>
                        <span class="text-base font-semibold text-foreground">{count}</span>
                    </div>
                }.into_any()
            })
            .collect::<Vec<_>>()}
    }
    .into_any()
}

fn render_recent_environments(environments: Vec<KubeEnvironment>) -> AnyView {
    if environments.is_empty() {
        return view! {
            <div class="flex flex-col items-start gap-3 rounded-lg border border-dashed border-border/70 px-6 py-8">
                <P class="font-medium text-foreground">"No recent activity yet"</P>
                <P class="text-sm text-muted-foreground">
                    "Start by deploying a personal environment from an app catalog or invite your team to collaborate on a shared one."
                </P>
                <a href="/kubernetes/catalogs">
                    <Button size=ButtonSize::Sm>
                        <lucide_leptos::Rocket attr:class="h-4 w-4" />
                        "Create your first environment"
                    </Button>
                </a>
            </div>
        }
        .into_any();
    }

    let rows = environments
        .into_iter()
        .map(render_environment_row)
        .collect::<Vec<_>>();

    view! {
        <Table>
            <TableHeader>
                <TableRow>
                    <TableHead>"Name"</TableHead>
                    <TableHead>"Type"</TableHead>
                    <TableHead>"Cluster"</TableHead>
                    <TableHead>"Status"</TableHead>
                    <TableHead>"Activity"</TableHead>
                    <TableHead class="text-right">"Actions"</TableHead>
                </TableRow>
            </TableHeader>
            <TableBody>
                {rows}
            </TableBody>
        </Table>
    }
    .into_any()
}

fn render_environment_row(environment: KubeEnvironment) -> AnyView {
    let env_type = environment_kind_badge(&environment);
    let status_variant = status_badge_variant(&environment.status);
    let status_label = environment.status.to_string();
    let (activity_label, activity_time) = last_activity(&environment);
    let activity_display = activity_time
        .as_ref()
        .map(|ts| format!("{} • {}", activity_label, format_timestamp(ts)))
        .unwrap_or_else(|| activity_label);
    let detail_href = format!("/kubernetes/environments/{}", environment.id);
    let catalog_href = format!("/kubernetes/catalogs/{}", environment.app_catalog_id);

    view! {
        <TableRow>
            <TableCell>
                <div class="flex flex-col gap-1">
                    <span class="font-medium text-foreground">{environment.name.clone()}</span>
                    <a href=catalog_href class="text-xs text-primary hover:underline">
                        {environment.app_catalog_name.clone()}
                    </a>
                </div>
            </TableCell>
            <TableCell>
                <Badge variant=env_type.1>{env_type.0}</Badge>
            </TableCell>
            <TableCell>
                <span class="text-sm text-muted-foreground">{environment.cluster_name.clone()}</span>
            </TableCell>
            <TableCell>
                <div class="flex flex-col gap-1">
                    <Badge variant=status_variant>{status_label}</Badge>
                    {if environment.catalog_update_available {
                        view! {
                            <Badge variant=BadgeVariant::Destructive class="uppercase tracking-wide">"Sync required"</Badge>
                        }.into_any()
                    } else {
                        view! { <></> }.into_any()
                    }}
                </div>
            </TableCell>
            <TableCell>
                <span class="text-sm text-muted-foreground">{activity_display}</span>
            </TableCell>
            <TableCell class="text-right">
                <a href=detail_href class="inline-flex">
                    <Button variant=ButtonVariant::Link size=ButtonSize::Sm>
                        "View details"
                        <lucide_leptos::ArrowRight attr:class="h-4 w-4" />
                    </Button>
                </a>
            </TableCell>
        </TableRow>
    }
    .into_any()
}

fn environment_kind_badge(environment: &KubeEnvironment) -> (&'static str, BadgeVariant) {
    if environment.base_environment_id.is_some() {
        ("Branch", BadgeVariant::Outline)
    } else if environment.is_shared {
        ("Shared", BadgeVariant::Secondary)
    } else {
        ("Personal", BadgeVariant::Default)
    }
}

fn status_badge_variant(status: &KubeEnvironmentStatus) -> BadgeVariant {
    match status {
        KubeEnvironmentStatus::Running => BadgeVariant::Secondary,
        KubeEnvironmentStatus::Creating
        | KubeEnvironmentStatus::Pausing
        | KubeEnvironmentStatus::Resuming => BadgeVariant::Outline,
        KubeEnvironmentStatus::Paused => BadgeVariant::Outline,
        KubeEnvironmentStatus::Deleting | KubeEnvironmentStatus::Deleted => BadgeVariant::Outline,
        KubeEnvironmentStatus::Failed
        | KubeEnvironmentStatus::Error
        | KubeEnvironmentStatus::PauseFailed
        | KubeEnvironmentStatus::ResumeFailed => BadgeVariant::Destructive,
    }
}

fn status_label(status: &KubeEnvironmentStatus) -> &'static str {
    match status {
        KubeEnvironmentStatus::Running => "Running",
        KubeEnvironmentStatus::Creating => "Provisioning",
        KubeEnvironmentStatus::Resuming => "Resuming",
        KubeEnvironmentStatus::Pausing => "Pausing",
        KubeEnvironmentStatus::Paused => "Paused",
        KubeEnvironmentStatus::Deleting | KubeEnvironmentStatus::Deleted => "Tearing down",
        KubeEnvironmentStatus::Failed
        | KubeEnvironmentStatus::Error
        | KubeEnvironmentStatus::PauseFailed
        | KubeEnvironmentStatus::ResumeFailed => "Needs attention",
    }
}

fn parse_datetime(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f %z")
        .or_else(|_| DateTime::parse_from_rfc3339(value))
        .ok()
}

fn last_activity(environment: &KubeEnvironment) -> (String, Option<DateTime<FixedOffset>>) {
    let mut events = Vec::with_capacity(4);
    if let Some(ts) = environment
        .resumed_at
        .as_ref()
        .and_then(|value| parse_datetime(value))
    {
        events.push((ts, "Resumed"));
    }
    if let Some(ts) = environment
        .paused_at
        .as_ref()
        .and_then(|value| parse_datetime(value))
    {
        events.push((ts, "Paused"));
    }
    if let Some(ts) = environment
        .last_catalog_synced_at
        .as_ref()
        .and_then(|value| parse_datetime(value))
    {
        events.push((ts, "Catalog synced"));
    }
    if let Some(ts) = parse_datetime(&environment.created_at) {
        events.push((ts, "Created"));
    }

    events
        .into_iter()
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(ts, label)| (label.to_string(), Some(ts)))
        .unwrap_or_else(|| ("Created".to_string(), None))
}

fn format_timestamp(ts: &DateTime<FixedOffset>) -> String {
    ts.format("%b %e, %Y • %H:%M %Z").to_string()
}

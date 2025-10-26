use std::{convert::Infallible, str::FromStr, sync::Arc, time::Duration};

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
};
use axum_extra::{headers, TypedHeader};
use chrono::Utc;
use futures::{stream, Stream, StreamExt};
use lapdev_common::kube::{ClusterStatusEvent, KubeClusterStatus};
use lapdev_rpc::error::ApiError;
use tokio_stream::wrappers::BroadcastStream;
use tracing::warn;
use uuid::Uuid;

use crate::state::CoreState;

pub async fn stream_cluster_status_events(
    Path((org_id, cluster_id)): Path<(Uuid, Uuid)>,
    State(state): State<Arc<CoreState>>,
    TypedHeader(cookies): TypedHeader<headers::Cookie>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let user = state.authenticate(&cookies).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;

    let cluster = state
        .db
        .get_kube_cluster(cluster_id)
        .await?
        .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

    if cluster.organization_id != org_id {
        return Err(ApiError::Unauthorized);
    }

    let status =
        KubeClusterStatus::from_str(&cluster.status).unwrap_or(KubeClusterStatus::Provisioning);
    let updated_at = cluster
        .last_reported_at
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let initial_event = ClusterStatusEvent {
        organization_id: cluster.organization_id,
        cluster_id,
        status,
        cluster_version: cluster.cluster_version.clone(),
        region: cluster.region.clone(),
        updated_at,
    };

    let receiver = state.cluster_events.subscribe();

    let initial_stream = stream::iter(
        build_cluster_sse_event(&initial_event)
            .into_iter()
            .map(Ok::<Event, Infallible>),
    );

    let target_org = org_id;
    let target_cluster = cluster_id;

    let event_stream = BroadcastStream::new(receiver).filter_map(move |result| async move {
        match result {
            Ok(event)
                if event.organization_id == target_org && event.cluster_id == target_cluster =>
            {
                build_cluster_sse_event(&event).map(Ok)
            }
            Ok(_) => None,
            Err(err) => {
                warn!("cluster event stream lagged: {err}");
                None
            }
        }
    });

    let stream = initial_stream.chain(event_stream);
    let keep_alive = KeepAlive::new()
        .interval(Duration::from_secs(15))
        .text("keep-alive");

    Ok(Sse::new(stream).keep_alive(keep_alive))
}

fn build_cluster_sse_event(event: &ClusterStatusEvent) -> Option<Event> {
    Event::default().event("cluster").json_data(event).ok()
}

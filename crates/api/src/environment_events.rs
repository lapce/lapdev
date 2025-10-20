use std::{convert::Infallible, str::FromStr, sync::Arc, time::Duration};

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
};
use axum_extra::{headers, TypedHeader};
use chrono::{DateTime, Utc};
use futures::{stream, Stream, StreamExt};
use lapdev_common::kube::KubeEnvironmentStatus;
use lapdev_rpc::error::ApiError;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::BroadcastStream;
use tracing::warn;
use uuid::Uuid;

use crate::state::CoreState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentLifecycleEvent {
    pub organization_id: Uuid,
    pub environment_id: Uuid,
    pub status: KubeEnvironmentStatus,
    pub paused_at: Option<String>,
    pub resumed_at: Option<String>,
    pub updated_at: DateTime<Utc>,
}

pub async fn stream_environment_events(
    Path((org_id, environment_id)): Path<(Uuid, Uuid)>,
    State(state): State<Arc<CoreState>>,
    TypedHeader(cookies): TypedHeader<headers::Cookie>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let user = state.authenticate(&cookies).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;

    let environment = state
        .db
        .get_kube_environment(environment_id)
        .await?
        .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

    if environment.organization_id != org_id {
        return Err(ApiError::Unauthorized);
    }

    let status = KubeEnvironmentStatus::from_str(&environment.status)
        .unwrap_or(KubeEnvironmentStatus::Pending);
    let initial_event = EnvironmentLifecycleEvent {
        organization_id: environment.organization_id,
        environment_id,
        status,
        paused_at: environment.paused_at.map(|dt| dt.to_string()),
        resumed_at: environment.resumed_at.map(|dt| dt.to_string()),
        updated_at: Utc::now(),
    };

    let receiver = state.environment_events.subscribe();

    let initial_stream = stream::iter(
        build_sse_event(&initial_event)
            .into_iter()
            .map(Ok::<Event, Infallible>),
    );

    let target_org = org_id;
    let target_env = environment_id;

    let event_stream = BroadcastStream::new(receiver).filter_map(move |result| {
        let target_org = target_org;
        let target_env = target_env;
        async move {
            match result {
                Ok(event)
                    if event.organization_id == target_org
                        && event.environment_id == target_env =>
                {
                    build_sse_event(&event).map(Ok)
                }
                Ok(_) => None,
                Err(err) => {
                    warn!("environment event stream lagged: {err}");
                    None
                }
            }
        }
    });

    let stream = initial_stream.chain(event_stream);
    let keep_alive = KeepAlive::new()
        .interval(Duration::from_secs(15))
        .text("keep-alive");

    Ok(Sse::new(stream).keep_alive(keep_alive))
}

fn build_sse_event(event: &EnvironmentLifecycleEvent) -> Option<Event> {
    Event::default().event("environment").json_data(event).ok()
}

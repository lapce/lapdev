use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
};
use axum_extra::{headers, TypedHeader};
use chrono::Utc;
use futures::{stream, Stream, StreamExt};
use lapdev_common::kube::AppCatalogStatusEvent;
use lapdev_rpc::error::ApiError;
use tokio_stream::wrappers::BroadcastStream;
use tracing::warn;
use uuid::Uuid;

use crate::state::CoreState;

pub async fn stream_app_catalog_events(
    Path((org_id, catalog_id)): Path<(Uuid, Uuid)>,
    State(state): State<Arc<CoreState>>,
    TypedHeader(cookies): TypedHeader<headers::Cookie>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let user = state.authenticate(&cookies).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;

    let catalog = state
        .db
        .get_app_catalog(catalog_id)
        .await?
        .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

    if catalog.organization_id != org_id {
        return Err(ApiError::Unauthorized);
    }

    let initial_event = AppCatalogStatusEvent {
        organization_id: catalog.organization_id,
        catalog_id,
        cluster_id: catalog.cluster_id,
        sync_version: catalog.sync_version,
        last_synced_at: catalog.last_synced_at.map(|dt| dt.to_string()),
        last_sync_actor_id: catalog.last_sync_actor_id,
        updated_at: catalog
            .last_synced_at
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now),
    };

    let receiver = state.app_catalog_events.subscribe();
    let initial_stream = stream::iter(
        build_app_catalog_sse_event(&initial_event)
            .into_iter()
            .map(Ok::<Event, Infallible>),
    );

    let target_org = org_id;
    let target_catalog = catalog_id;

    let event_stream = BroadcastStream::new(receiver).filter_map(move |result| async move {
        match result {
            Ok(event)
                if event.organization_id == target_org && event.catalog_id == target_catalog =>
            {
                build_app_catalog_sse_event(&event).map(Ok)
            }
            Ok(_) => None,
            Err(err) => {
                warn!("app catalog event stream lagged: {err}");
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

pub async fn stream_organization_app_catalog_events(
    Path(org_id): Path<Uuid>,
    State(state): State<Arc<CoreState>>,
    TypedHeader(cookies): TypedHeader<headers::Cookie>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let user = state.authenticate(&cookies).await?;
    state
        .db
        .get_organization_member(user.id, org_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?;

    let receiver = state.app_catalog_events.subscribe();
    let target_org = org_id;

    let event_stream = BroadcastStream::new(receiver).filter_map(move |result| {
        let target_org = target_org;
        async move {
            match result {
                Ok(event) if event.organization_id == target_org => {
                    build_app_catalog_sse_event(&event).map(Ok)
                }
                Ok(_) => None,
                Err(err) => {
                    warn!("app catalog event stream lagged: {err}");
                    None
                }
            }
        }
    });

    let keep_alive = KeepAlive::new()
        .interval(Duration::from_secs(15))
        .text("keep-alive");

    Ok(Sse::new(event_stream).keep_alive(keep_alive))
}

fn build_app_catalog_sse_event(event: &AppCatalogStatusEvent) -> Option<Event> {
    Event::default().event("catalog").json_data(event).ok()
}

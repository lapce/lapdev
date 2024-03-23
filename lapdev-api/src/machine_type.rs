use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::{headers::Cookie, TypedHeader};
use chrono::Utc;
use hyper::StatusCode;
use lapdev_common::{CreateMachineType, MachineType, UpdateMachineType};
use lapdev_db::entities;
use lapdev_rpc::error::ApiError;
use sea_orm::{ActiveModelTrait, ActiveValue};
use uuid::Uuid;

use crate::state::CoreState;

pub async fn all_machine_types(State(state): State<CoreState>) -> Result<Response, ApiError> {
    let machine_types = state.db.get_all_machine_types().await?;
    let machine_types: Vec<MachineType> = machine_types
        .into_iter()
        .map(|m| MachineType {
            id: m.id,
            name: m.name,
            shared: m.shared,
            cpu: m.cpu as usize,
            memory: m.memory as usize,
            disk: m.disk as usize,
            cost_per_second: m.cost_per_second as usize,
        })
        .collect();
    Ok(Json(machine_types).into_response())
}

pub async fn create_machine_type(
    TypedHeader(cookie): TypedHeader<Cookie>,
    State(state): State<CoreState>,
    Json(create_machine_type): Json<CreateMachineType>,
) -> Result<Json<MachineType>, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;

    if create_machine_type.cpu == 0 {
        return Err(ApiError::InvalidRequest("cpu can't be 0".to_string()));
    }
    if create_machine_type.memory == 0 {
        return Err(ApiError::InvalidRequest("memory can't be 0".to_string()));
    }
    if create_machine_type.disk == 0 {
        return Err(ApiError::InvalidRequest("disk can't be 0".to_string()));
    }

    let shared = if state.conductor.enterprise.has_valid_license().await {
        create_machine_type.shared
    } else {
        // shared cpu only supported by enterprise
        false
    };

    let machine_type = entities::machine_type::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        deleted_at: ActiveValue::Set(None),
        name: ActiveValue::Set(create_machine_type.name),
        cpu: ActiveValue::Set(create_machine_type.cpu as i32),
        shared: ActiveValue::Set(shared),
        memory: ActiveValue::Set(create_machine_type.memory as i32),
        disk: ActiveValue::Set(create_machine_type.disk as i32),
        cost_per_second: ActiveValue::Set(create_machine_type.cost_per_second as i32),
    }
    .insert(&state.db.conn)
    .await?;

    Ok(Json(MachineType {
        id: machine_type.id,
        name: machine_type.name,
        shared: machine_type.shared,
        cpu: machine_type.cpu as usize,
        memory: machine_type.memory as usize,
        disk: machine_type.disk as usize,
        cost_per_second: machine_type.cost_per_second as usize,
    }))
}

pub async fn update_machine_type(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(machine_type_id): Path<Uuid>,
    State(state): State<CoreState>,
    Json(update_machine_type): Json<UpdateMachineType>,
) -> Result<Json<MachineType>, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;

    let machine_type = entities::machine_type::ActiveModel {
        id: ActiveValue::Set(machine_type_id),
        name: ActiveValue::Set(update_machine_type.name),
        cost_per_second: ActiveValue::Set(update_machine_type.cost_per_second as i32),
        ..Default::default()
    }
    .update(&state.db.conn)
    .await?;

    Ok(Json(MachineType {
        id: machine_type.id,
        name: machine_type.name,
        shared: machine_type.shared,
        cpu: machine_type.cpu as usize,
        memory: machine_type.memory as usize,
        disk: machine_type.disk as usize,
        cost_per_second: machine_type.cost_per_second as usize,
    }))
}

pub async fn delete_machine_type(
    TypedHeader(cookie): TypedHeader<Cookie>,
    Path(machine_type_id): Path<Uuid>,
    State(state): State<CoreState>,
) -> Result<Response, ApiError> {
    state.authenticate_cluster_admin(&cookie).await?;

    let machine_types = state.db.get_all_machine_types().await?;
    if machine_types.len() == 1 {
        return Err(ApiError::InvalidRequest(
            "You can't delete the last machine type".to_string(),
        ));
    }

    entities::machine_type::ActiveModel {
        id: ActiveValue::Set(machine_type_id),
        deleted_at: ActiveValue::Set(Some(Utc::now().into())),
        ..Default::default()
    }
    .update(&state.db.conn)
    .await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

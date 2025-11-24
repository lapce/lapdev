use sea_orm_migration::{prelude::*, sea_orm::DbBackend};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::Postgres {
            return Ok(());
        }
        let conn = manager.get_connection();
        conn.execute_unprepared(
            r#"
                -- Add a table update notification function
                CREATE OR REPLACE FUNCTION table_update_notify() RETURNS trigger AS $$
                DECLARE
                id uuid;
                BEGIN
                IF TG_OP = 'INSERT' OR TG_OP = 'UPDATE' THEN
                    id = NEW.id;
                ELSE
                    id = OLD.id;
                END IF;
                PERFORM pg_notify('table_update', json_build_object('table', TG_TABLE_NAME, 'id', id, 'action_type', TG_OP)::text);
                RETURN NEW;
                END;
                $$ LANGUAGE plpgsql;

                -- Add row trigger
                CREATE TRIGGER workspace_host_notify_update AFTER INSERT or UPDATE or DELETE ON workspace_host FOR EACH ROW EXECUTE FUNCTION table_update_notify();
                
                -- Add a status update notification function
                CREATE OR REPLACE FUNCTION status_update_notify() RETURNS trigger AS $$
                BEGIN
                IF TG_OP = 'UPDATE' THEN
                    IF NEW.status != OLD.status THEN
                        PERFORM pg_notify('status_update', json_build_object('table', TG_TABLE_NAME, 'id', NEW.id, 'status', NEW.status, 'user_id', NEW.user_id)::text);
                    END IF;
                END IF;
                RETURN NEW;
                END;
                $$ LANGUAGE plpgsql;

                -- Add row trigger
                CREATE TRIGGER workspace_status_notify_update AFTER UPDATE ON workspace FOR EACH ROW EXECUTE FUNCTION status_update_notify();
                CREATE TRIGGER prebuild_status_notify_update AFTER UPDATE ON prebuild FOR EACH ROW EXECUTE FUNCTION status_update_notify();
                CREATE TRIGGER prebuild_replica_status_notify_update AFTER UPDATE ON prebuild_replica FOR EACH ROW EXECUTE FUNCTION status_update_notify();
                
                -- Add a config update notification function
                CREATE OR REPLACE FUNCTION config_update_notify() RETURNS trigger AS $$
                BEGIN
                IF TG_OP = 'INSERT' OR TG_OP = 'UPDATE' THEN
                    PERFORM pg_notify('config_update', json_build_object('name', NEW.name, 'value', NEW.value)::text);
                END IF;
                RETURN NEW;
                END;
                $$ LANGUAGE plpgsql;

                -- Add row trigger
                CREATE TRIGGER config_notify_update AFTER INSERT or UPDATE ON config FOR EACH ROW EXECUTE FUNCTION config_update_notify();
            "#,
        )
        .await?;
        Ok(())
    }
}

use std::future::Future;

use tokio::runtime::Handle;

pub(crate) fn spawn_detached<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    if Handle::try_current().is_ok() {
        tokio::spawn(future);
    }
}

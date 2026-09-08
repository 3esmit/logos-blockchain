use std::{
    fmt::{Debug, Display},
    time::Duration,
};

use lb_services_utils::wait_until_services_are_ready;
use overwatch::{DynError, overwatch::OverwatchHandle, services::AsServiceId};

pub async fn wait_for_dependencies<Kms, Sdp, Chain, RuntimeServiceId>(
    handle: &OverwatchHandle<RuntimeServiceId>,
) -> Result<(), DynError>
where
    RuntimeServiceId: AsServiceId<Kms>
        + AsServiceId<Sdp>
        + AsServiceId<Chain>
        + Debug
        + Display
        + Send
        + Sync
        + 'static,
{
    wait_until_services_are_ready!(handle, Some(Duration::from_mins(1)), Kms, Sdp).await?;
    // Persisted-chain recovery can exceed the ordinary startup deadline.
    // The service runner still owns cancellation of this readiness wait.
    wait_until_services_are_ready!(handle, None, Chain).await
}

#[cfg(test)]
mod tests;

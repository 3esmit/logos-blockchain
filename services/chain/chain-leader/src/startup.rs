use std::{
    fmt::{Debug, Display},
    time::Duration,
};

use lb_services_utils::wait_until_services_are_ready;
use overwatch::{DynError, overwatch::OverwatchHandle, services::AsServiceId};

pub async fn wait_for_dependencies<
    Mempool,
    Time,
    Wallet,
    Kms,
    Chain,
    ChainNetwork,
    Blend,
    RuntimeServiceId,
>(
    handle: &OverwatchHandle<RuntimeServiceId>,
) -> Result<(), DynError>
where
    RuntimeServiceId: AsServiceId<Mempool>
        + AsServiceId<Time>
        + AsServiceId<Wallet>
        + AsServiceId<Kms>
        + AsServiceId<Chain>
        + AsServiceId<ChainNetwork>
        + AsServiceId<Blend>
        + Debug
        + Display
        + Send
        + Sync
        + 'static,
{
    wait_until_services_are_ready!(handle, Some(Duration::from_mins(1)), Mempool, Time, Kms)
        .await?;
    // Wallet waits for chain recovery; it must share the recovery-dependent
    // readiness phase, together with IBD and the Blend online transition.
    wait_until_services_are_ready!(handle, None, Wallet, Chain, ChainNetwork, Blend).await
}

#[cfg(test)]
mod tests;

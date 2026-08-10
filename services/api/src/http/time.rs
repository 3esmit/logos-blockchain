use std::fmt::{Debug, Display};

use lb_http_api_common::TimeInfo;
use lb_time_service::TimeServiceMessage;
use overwatch::{
    overwatch::handle::OverwatchHandle,
    services::{AsServiceId, ServiceData},
};
use tokio::sync::oneshot;

use crate::http::DynError;

fn map_time_info(service_info: &lb_time_service::TimeServiceInfo) -> TimeInfo {
    TimeInfo {
        slot_duration_ms: service_info.slot_duration_ms,
        genesis_time_unix_ms: service_info.genesis_time_unix_ms,
        current_slot: u64::from(service_info.current_slot),
        current_epoch: u32::from(service_info.current_epoch),
    }
}

/// Returns the current time-service snapshot in the same shape as `/time/info`.
pub async fn time_info<TimeService, RuntimeServiceId>(
    handle: &OverwatchHandle<RuntimeServiceId>,
) -> Result<TimeInfo, DynError>
where
    TimeService: ServiceData<Message = TimeServiceMessage>,
    RuntimeServiceId: Debug + Send + Sync + Display + 'static + AsServiceId<TimeService>,
{
    let relay = handle.relay::<TimeService>().await?;
    let (sender, receiver) = oneshot::channel();
    relay
        .send(TimeServiceMessage::Info { sender })
        .await
        .map_err(|_| std::io::Error::other("time service relay is closed"))?;
    let service_info = receiver.await?.map_err(std::io::Error::other)?;

    Ok(map_time_info(&service_info))
}

#[cfg(test)]
mod tests {
    use lb_chain_service::{Epoch, Slot};
    use lb_time_service::TimeServiceInfo;

    use super::map_time_info;

    #[test]
    fn time_info_preserves_the_service_snapshot_fields() {
        let service_info = TimeServiceInfo {
            slot_duration_ms: 1_000,
            genesis_time_unix_ms: 1_700_000_000_000,
            current_slot: Slot::new(42),
            current_epoch: Epoch::new(7),
        };

        let info = map_time_info(&service_info);

        assert_eq!(info.slot_duration_ms, 1_000);
        assert_eq!(info.genesis_time_unix_ms, 1_700_000_000_000);
        assert_eq!(info.current_slot, 42);
        assert_eq!(info.current_epoch, 7);
    }
}

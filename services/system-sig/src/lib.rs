use std::fmt::{Debug, Display};

use lb_log_targets::system_sig;
use overwatch::{
    DynError, OpaqueServiceResourcesHandle,
    overwatch::handle::OverwatchHandle,
    services::{
        AsServiceId, ServiceCore, ServiceData,
        state::{NoOperator, NoState},
    },
};

const LOG_TARGET: &str = system_sig::ROOT;

pub struct SystemSig<RuntimeServiceId> {
    service_resources_handle: OpaqueServiceResourcesHandle<Self, RuntimeServiceId>,
}

#[derive(Debug)]
pub enum SystemSigMessage {
    Shutdown,
}

impl<RuntimeServiceId> SystemSig<RuntimeServiceId>
where
    RuntimeServiceId: Debug + Display + Sync,
{
    async fn ctrl_c_signal_received(overwatch_handle: &OverwatchHandle<RuntimeServiceId>) {
        tracing::debug!(target: LOG_TARGET, "Ctrl-C received, requesting shutdown");
        drop(overwatch_handle.shutdown().await);
    }
}

impl<RuntimeServiceId> ServiceData for SystemSig<RuntimeServiceId> {
    const SERVICE_RELAY_BUFFER_SIZE: usize = 1;
    type Settings = ();
    type State = NoState<Self::Settings>;
    type StateOperator = NoOperator<Self::State>;
    type Message = SystemSigMessage;
}

#[async_trait::async_trait]
impl<RuntimeServiceId> ServiceCore<RuntimeServiceId> for SystemSig<RuntimeServiceId>
where
    RuntimeServiceId: Debug + Display + Sync + Send + Clone + AsServiceId<Self> + 'static,
{
    fn init(
        service_resources_handle: OpaqueServiceResourcesHandle<Self, RuntimeServiceId>,
        _initial_state: Self::State,
    ) -> Result<Self, DynError> {
        Ok(Self {
            service_resources_handle,
        })
    }

    async fn run(self) -> Result<(), DynError> {
        let service_resources_handle = self.service_resources_handle;
        let overwatch_handle = service_resources_handle.overwatch_handle.clone();
        let status_updater = service_resources_handle.status_updater;
        let mut inbound_relay = service_resources_handle.inbound_relay;
        let ctrl_c = async_ctrlc::CtrlC::new()?;

        status_updater.notify_ready();
        tracing::info!(
            target: LOG_TARGET,
            "Service '{}' is ready.",
            <RuntimeServiceId as AsServiceId<Self>>::SERVICE_ID
        );

        tokio::select! {
            () = ctrl_c => Self::ctrl_c_signal_received(&overwatch_handle).await,
            Some(SystemSigMessage::Shutdown) = inbound_relay.recv() => {
                tracing::debug!(target: LOG_TARGET, "Shutdown requested by a service failure");
                tokio::spawn(async move {
                    if let Err(error) = overwatch_handle.shutdown().await {
                        tracing::error!(target: LOG_TARGET, "Failed to shut down Overwatch: {error:?}");
                    }
                });
            }
        }

        Ok(())
    }
}

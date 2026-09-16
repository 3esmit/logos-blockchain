use futures::{Stream, StreamExt as _};
use lb_core::mantle::mock::{MockTransaction, MockTxId};
use lb_log_targets::mempool;
use lb_network_service::{
    NetworkService,
    api::NetworkServiceApi,
    backends::mock::{Mock, MockBackendMessage, MockContentTopic, MockMessage, NetworkEvent},
};
use overwatch::services::{ServiceData, relay::OutboundRelay};

use crate::network::NetworkAdapter;

pub const MOCK_PUB_SUB_TOPIC: &str = "MockPubSubTopic";
pub const MOCK_TX_CONTENT_TOPIC: MockContentTopic = MockContentTopic::new("Mock", 1, "Tx");

const LOG_TARGET: &str = mempool::network::ROOT;

pub struct MockAdapter<RuntimeServiceId> {
    network_api: NetworkServiceApi<Mock, RuntimeServiceId>,
}

#[async_trait::async_trait]
impl<RuntimeServiceId> NetworkAdapter<RuntimeServiceId> for MockAdapter<RuntimeServiceId> {
    type Backend = Mock;
    type Settings = ();
    type Payload = MockTransaction<MockMessage>;
    type Key = MockTxId;

    async fn new(
        _settings: Self::Settings,
        network_relay: OutboundRelay<
            <NetworkService<Self::Backend, RuntimeServiceId> as ServiceData>::Message,
        >,
    ) -> Self {
        // send message to boot the network producer
        let network_api = NetworkServiceApi::new(network_relay);
        if let Err(e) = network_api
            .process(MockBackendMessage::BootProducer {
                spawner: Box::new(move |fut| {
                    tokio::spawn(fut);
                    Ok(())
                }),
            })
            .await
        {
            panic!("Couldn't send boot producer message to the network service: {e:?}");
        }

        if let Err(e) = network_api
            .process(MockBackendMessage::RelaySubscribe {
                topic: MOCK_PUB_SUB_TOPIC.to_owned(),
            })
            .await
        {
            panic!("Couldn't send subscribe message to the network service: {e}");
        }
        Self { network_api }
    }

    async fn payload_stream(
        &self,
    ) -> Box<dyn Stream<Item = (Self::Key, Self::Payload)> + Unpin + Send> {
        let stream = self
            .network_api
            .subscribe_to_pubsub()
            .await
            .expect("Network backend should be ready");
        Box::new(Box::pin(stream.filter_map(async |event| match event {
            Ok(NetworkEvent::RawMessage(message)) => {
                tracing::debug!(target: LOG_TARGET, "Received message: {:?}", message.payload());
                message.content_topic().eq(&MOCK_TX_CONTENT_TOPIC).then(|| {
                    let tx = MockTransaction::new(message);
                    (tx.id(), tx)
                })
            }
            Err(_e) => None,
        })))
    }

    async fn send(&self, msg: Self::Payload) {
        if let Err(e) = self
            .network_api
            .process(MockBackendMessage::Broadcast {
                topic: MOCK_PUB_SUB_TOPIC.into(),
                msg: msg.message().clone(),
            })
            .await
        {
            tracing::error!(target: LOG_TARGET, "failed to send item to topic: {e}");
        }
    }
}

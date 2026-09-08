use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use lb_blend::scheduling::epoch::UninitializedEpochEventStream;
use overwatch::overwatch::OverwatchHandle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_stream::wrappers::ReceiverStream;

use super::{
    test_blend_epoch_state,
    utils::{MockLeaderProofsGenerator, NodeId, TestBackend, overwatch_handle, settings},
};
use crate::{
    edge::{messages_to_blend, run},
    epoch_info::{PolEpochInfo, PolInfoProvider},
    message::ServiceMessage,
    test_utils::{membership::membership, network::TestNetworkAdapter},
};

struct PendingPolSubscription;
struct PendingPolEpoch;

#[async_trait]
impl PolInfoProvider<usize> for PendingPolSubscription {
    type Stream = stream::Pending<PolEpochInfo>;

    async fn subscribe(_: &OverwatchHandle<usize>) -> Option<Self::Stream> {
        std::future::pending().await
    }
}

#[async_trait]
impl PolInfoProvider<usize> for PendingPolEpoch {
    type Stream = stream::Pending<PolEpochInfo>;

    async fn subscribe(_: &OverwatchHandle<usize>) -> Option<Self::Stream> {
        Some(stream::pending())
    }
}

async fn network_info_after_ready<Provider>()
where
    Provider: PolInfoProvider<usize, Stream: Stream<Item = PolEpochInfo> + Send + Unpin>,
{
    let local_node = NodeId(99);
    let (node_sender, _node_receiver) = mpsc::channel(1);
    let (message_sender, message_receiver) = mpsc::channel(1);
    let (ready_sender, mut ready_receiver) = watch::channel(false);
    let (network, _broadcasts) = TestNetworkAdapter::new();
    let epoch = test_blend_epoch_state(0.into(), membership(&[NodeId(0)], local_node));
    let task = tokio::spawn(async move {
        Box::pin(run::<
            TestBackend,
            _,
            _,
            MockLeaderProofsGenerator,
            Provider,
            _,
        >(
            UninitializedEpochEventStream::new(stream::iter([epoch]), Duration::ZERO),
            Box::pin(messages_to_blend(
                ReceiverStream::new(message_receiver),
                local_node,
            )),
            network,
            settings(local_node, 1, node_sender),
            &overwatch_handle(),
            || {
                ready_sender.send_replace(true);
            },
        ))
        .await
    });
    drop(ready_receiver.wait_for(|ready| *ready).await.unwrap());
    let (reply, response) = oneshot::channel();
    message_sender
        .send(ServiceMessage::GetNetworkInfo { reply })
        .await
        .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(1), response).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let info = received
        .expect("Ready edge must serve network info without a PoL subscription")
        .expect("Network info sender remains alive")
        .expect("Edge network info is available");
    assert_eq!(info.node_id, local_node);
    assert!(info.core_info.is_none());
}

#[tokio::test(start_paused = true)]
async fn network_info_does_not_wait_for_pol_subscription() {
    network_info_after_ready::<PendingPolSubscription>().await;
}

#[tokio::test(start_paused = true)]
async fn network_info_does_not_wait_for_first_pol_epoch() {
    network_info_after_ready::<PendingPolEpoch>().await;
}

#[tokio::test(start_paused = true)]
async fn membership_change_can_stop_edge_during_pol_subscription() {
    let local_node = NodeId(99);
    let (node_sender, _node_receiver) = mpsc::channel(1);
    let (epoch_sender, epoch_receiver) = mpsc::channel(1);
    let (ready_sender, mut ready_receiver) = watch::channel(false);
    let (network, _broadcasts) = TestNetworkAdapter::new();
    epoch_sender
        .send(test_blend_epoch_state(
            0.into(),
            membership(&[NodeId(0)], local_node),
        ))
        .await
        .unwrap();
    let mut task = tokio::spawn(async move {
        Box::pin(run::<
            TestBackend,
            _,
            _,
            MockLeaderProofsGenerator,
            PendingPolSubscription,
            _,
        >(
            UninitializedEpochEventStream::new(ReceiverStream::new(epoch_receiver), Duration::ZERO),
            stream::pending(),
            network,
            settings(local_node, 1, node_sender),
            &overwatch_handle(),
            || {
                ready_sender.send_replace(true);
            },
        ))
        .await
    });
    drop(ready_receiver.wait_for(|ready| *ready).await.unwrap());
    epoch_sender
        .send(test_blend_epoch_state(
            1.into(),
            membership(&[], local_node),
        ))
        .await
        .unwrap();
    let finished = tokio::time::timeout(Duration::from_secs(1), &mut task).await;
    if finished.is_err() {
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    }
    assert!(matches!(
        finished.expect("Membership loss must end the edge service"),
        Ok(Ok(()))
    ));
}

use core::{
    ops::RangeInclusive,
    task::{Context, Poll, Waker},
};
use std::{collections::VecDeque, io};

use futures::{FutureExt as _, future::BoxFuture};
use libp2p::{
    Stream, StreamProtocol,
    core::upgrade::ReadyUpgrade,
    swarm::{
        ConnectionHandlerEvent, SubstreamProtocol,
        handler::{
            ConnectionEvent, DialUpgradeError, FullyNegotiatedInbound, FullyNegotiatedOutbound,
        },
    },
};

use crate::{
    core::with_core::behaviour::handler::conn_maintenance::{
        ConnectionMonitor, ConnectionMonitorOutput,
    },
    recv_msg, send_msg,
};

pub(super) mod conn_maintenance;

// Metrics
const VALUE_FULLY_NEGOTIATED_INBOUND: &str = "fully_negotiated_inbound";
const VALUE_FULLY_NEGOTIATED_OUTBOUND: &str = "fully_negotiated_outbound";
const VALUE_DIAL_UPGRADE_ERROR: &str = "dial_upgrade_error";
const VALUE_IGNORED: &str = "ignored";

const LOG_TARGET: &str = "blend::network::core::core::conn::handler";

pub struct ConnectionHandler<ConnectionWindowClock> {
    inbound_substream: Option<InboundSubstreamState>,
    outbound_substream: Option<OutboundSubstreamState>,
    outbound_msgs: VecDeque<Vec<u8>>,
    pending_events_to_behaviour: VecDeque<ToBehaviour>,
    monitor: ConnectionMonitor<ConnectionWindowClock>,
    protocol_name: StreamProtocol,
    waker: Option<Waker>,
}

type MsgSendFuture = BoxFuture<'static, Result<Stream, io::Error>>;
type MsgRecvFuture = BoxFuture<'static, Result<(Stream, Vec<u8>), io::Error>>;

enum InboundSubstreamState {
    /// A message is being received on the inbound substream.
    PendingRecv(MsgRecvFuture),
    /// A substream has been dropped proactively.
    Dropped,
}

enum OutboundSubstreamState {
    /// A request to open a new outbound substream is being processed.
    PendingOpenSubstream,
    /// An outbound substream is open and ready to send messages.
    Idle(Stream),
    /// A message is being sent on the outbound substream.
    PendingSend(MsgSendFuture),
    /// A substream has been dropped proactively.
    Dropped,
}

impl<ConnectionWindowClock> ConnectionHandler<ConnectionWindowClock> {
    pub fn new(
        monitor: ConnectionMonitor<ConnectionWindowClock>,
        protocol_name: StreamProtocol,
    ) -> Self {
        tracing::trace!(target: LOG_TARGET, "Initializing core->core connection handler.");
        Self {
            inbound_substream: None,
            outbound_substream: None,
            outbound_msgs: VecDeque::new(),
            pending_events_to_behaviour: VecDeque::new(),
            monitor,
            protocol_name,
            waker: None,
        }
    }

    /// Mark the inbound/outbound substream state as Dropped.
    /// Then the substream hold by the state will be dropped from memory.
    /// As a result, Swarm will decrease the ref count to the connection,
    /// and close the connection when the count is 0.
    ///
    /// Also, this clears all pending messages and events
    /// to avoid confusions for event recipients.
    fn close_substreams(&mut self) {
        self.inbound_substream = Some(InboundSubstreamState::Dropped);
        self.outbound_substream = Some(OutboundSubstreamState::Dropped);
        self.outbound_msgs.clear();
        self.pending_events_to_behaviour.clear();
    }

    fn try_wake(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

#[derive(Debug)]
pub enum FromBehaviour {
    /// A message to be sent to the connection.
    Message(Vec<u8>),
    /// Close inbound/outbound substreams.
    /// This happens when [`crate::Behaviour`] determines that one of the
    /// followings is true.
    /// - Max peering degree is reached.
    /// - The peer has been detected as spammy.
    CloseSubstreams,
}

#[derive(Debug)]
pub enum ToBehaviour {
    /// An inbound substream has been successfully upgraded for the blend
    /// protocol.
    FullyNegotiatedInbound,
    /// An outbound substream has been successfully upgraded for the blend
    /// protocol.
    FullyNegotiatedOutbound,
    /// An outbound substream was failed to be upgraded for the blend protocol.
    DialUpgradeError(DialUpgradeError<(), ReadyUpgrade<StreamProtocol>>),
    /// A message has been received from the connection.
    Message(Vec<u8>),
    /// Notifying that the peer is detected as spammy.
    /// The inbound/outbound streams to the peer are closed proactively.
    SpammyPeer,
    /// Notifying that the peer is detected as unhealthy.
    UnhealthyPeer,
    /// Notifying that the peer is detected as healthy.
    HealthyPeer,
    /// An IO error from the connection.
    /// The inbound/outbound streams to the peer are closed proactively.
    IOError(io::Error),
}

impl<ConnectionWindowClock> libp2p::swarm::ConnectionHandler
    for ConnectionHandler<ConnectionWindowClock>
where
    ConnectionWindowClock: futures::Stream<Item = RangeInclusive<u64>> + Unpin + Send + 'static,
{
    type FromBehaviour = FromBehaviour;
    type ToBehaviour = ToBehaviour;
    type InboundProtocol = ReadyUpgrade<StreamProtocol>;
    type InboundOpenInfo = ();
    type OutboundProtocol = ReadyUpgrade<StreamProtocol>;
    type OutboundOpenInfo = ();

    #[expect(deprecated, reason = "Self::InboundOpenInfo is deprecated")]
    fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, Self::InboundOpenInfo> {
        SubstreamProtocol::new(ReadyUpgrade::new(self.protocol_name.clone()), ())
    }

    #[expect(deprecated, reason = "Self::OutboundOpenInfo is deprecated")]
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<
        ConnectionHandlerEvent<Self::OutboundProtocol, Self::OutboundOpenInfo, Self::ToBehaviour>,
    > {
        tracing::trace!(gauge.pending_outbound_messages = self.outbound_msgs.len() as u64,);
        tracing::trace!(
            gauge.pending_events_to_behaviour = self.pending_events_to_behaviour.len() as u64,
        );

        // Short-circuit so that we do not poll the connection monitor anymore in case
        // either of the two substreams has been dropped.
        if matches!(self.inbound_substream, Some(InboundSubstreamState::Dropped))
            || matches!(
                self.outbound_substream,
                Some(OutboundSubstreamState::Dropped)
            )
        {
            return Poll::Pending;
        }

        // Check if the monitor interval has elapsed, if exists.
        self.poll_monitor(cx);

        // Process pending events to be sent to the behaviour
        if let Poll::Ready(event) = self.poll_pending_behaviour_events() {
            return Poll::Ready(event);
        }

        // Process inbound stream
        if let Poll::Ready(event) = self.poll_inbound_substream(cx) {
            return Poll::Ready(event);
        }

        // Process outbound stream
        self.poll_outbound_substream(cx)
    }

    fn on_behaviour_event(&mut self, event: Self::FromBehaviour) {
        match event {
            FromBehaviour::Message(msg) => {
                self.outbound_msgs.push_back(msg);
            }
            FromBehaviour::CloseSubstreams => {
                self.close_substreams();
            }
        }
    }

    #[expect(
        deprecated,
        reason = "Self::InboundOpenInfo and Self::OutboundOpenInfo are deprecated"
    )]
    fn on_connection_event(
        &mut self,
        event: ConnectionEvent<
            Self::InboundProtocol,
            Self::OutboundProtocol,
            Self::InboundOpenInfo,
            Self::OutboundOpenInfo,
        >,
    ) {
        let event_name = match event {
            ConnectionEvent::FullyNegotiatedInbound(FullyNegotiatedInbound {
                protocol: stream,
                ..
            }) => self.on_fully_negotiated_inbound(stream),
            ConnectionEvent::FullyNegotiatedOutbound(FullyNegotiatedOutbound {
                protocol: stream,
                ..
            }) => self.on_fully_negotiated_outbound(stream),
            ConnectionEvent::DialUpgradeError(e) => self.on_dial_upgrade_error(e),
            event => {
                tracing::trace!(target: LOG_TARGET, ?event, "Ignoring connection event");
                VALUE_IGNORED
            }
        };

        tracing::trace!(counter.connection_event = 1, event = event_name);
        self.try_wake();
    }
}

impl<ConnectionWindowClock> ConnectionHandler<ConnectionWindowClock>
where
    ConnectionWindowClock: futures::Stream<Item = RangeInclusive<u64>> + Unpin,
{
    fn poll_monitor(&mut self, cx: &mut Context<'_>) {
        // Check if the monitor interval has elapsed, if exists.
        if let Poll::Ready(output) = self.monitor.poll(cx) {
            match output {
                Some(ConnectionMonitorOutput::Spammy) => {
                    // TODO: Re-enable this once we have fixed Blend observation
                    // window range values.
                    // self.close_substreams();
                    self.pending_events_to_behaviour
                        .push_back(ToBehaviour::SpammyPeer);
                }
                Some(ConnectionMonitorOutput::Unhealthy) => {
                    self.pending_events_to_behaviour
                        .push_back(ToBehaviour::UnhealthyPeer);
                }
                Some(ConnectionMonitorOutput::Healthy) => {
                    self.pending_events_to_behaviour
                        .push_back(ToBehaviour::HealthyPeer);
                }
                None => panic!("Connection monitor stream was closed."),
            }
        }
    }

    fn poll_pending_behaviour_events(
        &mut self,
    ) -> Poll<ConnectionHandlerEvent<ReadyUpgrade<StreamProtocol>, (), ToBehaviour>> {
        self.pending_events_to_behaviour.pop_front().map_or_else(
            || Poll::Pending,
            |event| Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(event)),
        )
    }

    fn poll_inbound_substream(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ConnectionHandlerEvent<ReadyUpgrade<StreamProtocol>, (), ToBehaviour>> {
        match self.inbound_substream.take() {
            None => Poll::Pending,
            Some(InboundSubstreamState::PendingRecv(mut msg_recv_fut)) => match msg_recv_fut
                .poll_unpin(cx)
            {
                Poll::Ready(Ok((stream, msg))) => {
                    tracing::trace!(
                        target: LOG_TARGET,
                        "Received message from inbound stream; notifying behaviour"
                    );
                    // Record the message to the monitor.
                    self.monitor.record_message();
                    self.inbound_substream =
                        Some(InboundSubstreamState::PendingRecv(recv_msg(stream).boxed()));
                    // Notify behaviour.
                    Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(
                        ToBehaviour::Message(msg),
                    ))
                }
                Poll::Ready(Err(e)) => {
                    tracing::error!(target: LOG_TARGET, "Failed to receive message from inbound stream: {e:?}. Dropping both inbound/outbound substreams");
                    self.close_substreams();
                    Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(
                        ToBehaviour::IOError(e),
                    ))
                }
                Poll::Pending => {
                    self.inbound_substream = Some(InboundSubstreamState::PendingRecv(msg_recv_fut));
                    Poll::Pending
                }
            },
            Some(InboundSubstreamState::Dropped) => {
                self.inbound_substream = Some(InboundSubstreamState::Dropped);
                Poll::Pending
            }
        }
    }

    fn poll_outbound_substream(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ConnectionHandlerEvent<ReadyUpgrade<StreamProtocol>, (), ToBehaviour>> {
        loop {
            match self.outbound_substream.take() {
                // If the request to open a new outbound substream is still being processed, wait
                // more.
                Some(OutboundSubstreamState::PendingOpenSubstream) => {
                    return self.handle_pending_open_substream(cx);
                }
                // If the substream is idle, and if it's time to send a message, send it.
                Some(OutboundSubstreamState::Idle(stream)) => {
                    if let Some(output) = self.handle_idle_outbound_substream(stream, cx) {
                        return output;
                    }
                }
                // If a message is being sent, check if it's done.
                Some(OutboundSubstreamState::PendingSend(msg_send_fut)) => {
                    if let Some(output) = self.handle_pending_send_substream(msg_send_fut, cx) {
                        return output;
                    }
                }
                Some(OutboundSubstreamState::Dropped) => {
                    return self.handle_dropped_outbound_substream();
                }
                // If there is no outbound substream, request to open a new one.
                None => {
                    return Poll::Ready(self.request_outbound_substream());
                }
            }
        }
    }

    fn handle_pending_open_substream(
        &mut self,
        cx: &Context<'_>,
    ) -> Poll<ConnectionHandlerEvent<ReadyUpgrade<StreamProtocol>, (), ToBehaviour>> {
        self.outbound_substream = Some(OutboundSubstreamState::PendingOpenSubstream);
        self.waker = Some(cx.waker().clone());
        Poll::Pending
    }

    fn handle_idle_outbound_substream(
        &mut self,
        stream: Stream,
        cx: &Context<'_>,
    ) -> Option<Poll<ConnectionHandlerEvent<ReadyUpgrade<StreamProtocol>, (), ToBehaviour>>> {
        if let Some(msg) = self.outbound_msgs.pop_front() {
            tracing::trace!(target: LOG_TARGET, "Sending message to outbound stream");
            self.outbound_substream = Some(OutboundSubstreamState::PendingSend(
                send_msg(stream, msg).boxed(),
            ));
            return None;
        }

        self.outbound_substream = Some(OutboundSubstreamState::Idle(stream));
        self.waker = Some(cx.waker().clone());
        Some(Poll::Pending)
    }

    fn handle_pending_send_substream(
        &mut self,
        mut msg_send_fut: MsgSendFuture,
        cx: &mut Context<'_>,
    ) -> Option<Poll<ConnectionHandlerEvent<ReadyUpgrade<StreamProtocol>, (), ToBehaviour>>> {
        match msg_send_fut.poll_unpin(cx) {
            Poll::Ready(Ok(stream)) => {
                tracing::trace!(target: LOG_TARGET, "Message sent to outbound stream");
                self.outbound_substream = Some(OutboundSubstreamState::Idle(stream));
                None
            }
            Poll::Ready(Err(e)) => {
                tracing::error!(target: LOG_TARGET, "Failed to send message to outbound stream: {e:?}. Dropping both inbound and outbound substreams");
                self.close_substreams();
                Some(Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(
                    ToBehaviour::IOError(e),
                )))
            }
            Poll::Pending => {
                self.outbound_substream = Some(OutboundSubstreamState::PendingSend(msg_send_fut));
                self.waker = Some(cx.waker().clone());
                Some(Poll::Pending)
            }
        }
    }

    fn handle_dropped_outbound_substream(
        &mut self,
    ) -> Poll<ConnectionHandlerEvent<ReadyUpgrade<StreamProtocol>, (), ToBehaviour>> {
        tracing::trace!(target: LOG_TARGET, "Outbound substream dropped proactively");
        self.outbound_substream = Some(OutboundSubstreamState::Dropped);
        Poll::Pending
    }

    fn request_outbound_substream(
        &mut self,
    ) -> ConnectionHandlerEvent<ReadyUpgrade<StreamProtocol>, (), ToBehaviour> {
        tracing::trace!(
            target: LOG_TARGET,
            "Outbound substream not initialized yet; requesting swarm to open one"
        );
        self.outbound_substream = Some(OutboundSubstreamState::PendingOpenSubstream);
        ConnectionHandlerEvent::OutboundSubstreamRequest {
            protocol: SubstreamProtocol::new(ReadyUpgrade::new(self.protocol_name.clone()), ()),
        }
    }

    fn on_fully_negotiated_inbound(&mut self, stream: Stream) -> &'static str {
        tracing::trace!(target: LOG_TARGET, "Fully negotiated inbound; creating inbound substream");
        self.inbound_substream = Some(InboundSubstreamState::PendingRecv(recv_msg(stream).boxed()));
        self.pending_events_to_behaviour
            .push_back(ToBehaviour::FullyNegotiatedInbound);
        VALUE_FULLY_NEGOTIATED_INBOUND
    }

    fn on_fully_negotiated_outbound(&mut self, stream: Stream) -> &'static str {
        tracing::trace!(target: LOG_TARGET, "Fully negotiated outbound; creating outbound substream");
        self.outbound_substream = Some(OutboundSubstreamState::Idle(stream));
        self.pending_events_to_behaviour
            .push_back(ToBehaviour::FullyNegotiatedOutbound);
        VALUE_FULLY_NEGOTIATED_OUTBOUND
    }

    fn on_dial_upgrade_error(
        &mut self,
        error: DialUpgradeError<(), ReadyUpgrade<StreamProtocol>>,
    ) -> &'static str {
        tracing::error!(target: LOG_TARGET, "DialUpgradeError: {:?}", error);
        self.pending_events_to_behaviour
            .push_back(ToBehaviour::DialUpgradeError(error));
        self.close_substreams();
        VALUE_DIAL_UPGRADE_ERROR
    }
}

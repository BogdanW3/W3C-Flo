use crate::controller::{ControllerClient, SendWs, UpdateMuteList};
use crate::error::*;
use crate::game::local_game_from_game_info;
//use crate::game::LocalGameInfo;
use crate::message::messages;
use crate::message::messages::{ConnectRetrying, OutgoingMessage};
use crate::node::{AddNode, GetNodePingMap, NodeRegistry, RemoveNode, UpdateNodes};
use crate::ping::PingUpdate;
use crate::platform::{CalcMapChecksum, GetClientPlatformInfo, Platform};
use flo_net::packet::*;
use flo_net::proto::flo_connect as proto;
use flo_net::stream::FloStream;
use flo_state::{async_trait, Actor, Addr, Context, Handler, Message};
use flo_types::game::*;
use s2_grpc_utils::S2ProtoPack;
use s2_grpc_utils::{S2ProtoEnum, S2ProtoUnpack};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing_futures::Instrument;

struct ConnectionResult {
  result: Result<()>,
  duration: Duration,
}

enum ConnectionAction {
  Continue(String), // Includes reason for retry
  Break,
}

pub struct ControllerStream {
  id: u64,
  domain: String,
  token: String,
  parent: Addr<ControllerClient>,
  frame_tx: Sender<Frame>,
  frame_rx: Option<Receiver<Frame>>,
  current_game_info: Option<Arc<LocalGameInfo>>,
  platform: Addr<Platform>,
  nodes: Addr<NodeRegistry>,
  is_shutting_down: Arc<AtomicBool>,
}

impl ControllerStream {
  pub fn new(
    parent: Addr<ControllerClient>,
    platform: Addr<Platform>,
    nodes: Addr<NodeRegistry>,
    id: u64,
    domain: &str,
    token: String,
  ) -> Self {
    let (frame_tx, frame_rx) = channel(5);
    Self {
      id,
      domain: domain.to_string(),
      token: token.to_string(),
      parent,
      frame_tx,
      frame_rx: Some(frame_rx),
      current_game_info: None,
      platform,
      nodes,
      is_shutting_down: Arc::new(AtomicBool::new(false)),
    }
  }

  async fn run_connection_worker(
    id: u64,
    domain: String,
    token: String,
    owner: Addr<Self>,
    parent: Addr<ControllerClient>,
    nodes: Addr<NodeRegistry>,
    is_shutting_down: Arc<AtomicBool>,
  ) {
    let mut iteration_count = 0u32;
    let mut retry_count = 0u32;
    let mut ping_cancel_token: Option<CancellationToken> = None;

    loop {
      iteration_count += 1;

      tracing::info!(
        iteration = iteration_count,
        retry = retry_count,
        "starting connection attempt"
      );

      // Setup for this connection attempt
      if let Some(setup_result) =
        Self::setup_connection_attempt(&mut ping_cancel_token, id, &owner, &parent, &nodes).await
      {
        match setup_result {
          Ok(frame_rx) => {
            // Execute the connection attempt
            let outcome = Self::execute_connection_attempt(
              id,
              &domain,
              &token,
              frame_rx,
              &owner,
              &parent,
              &nodes,
              iteration_count,
              retry_count,
            )
            .await;

            // Handle the connection outcome
            match Self::handle_connection_outcome(
              outcome,
              &mut retry_count,
              &parent,
              &is_shutting_down,
              id,
            )
            .await
            {
              ConnectionAction::Continue(reason) => {
                // Calculate and wait for retry delay
                Self::wait_for_retry(id, &mut retry_count, &parent, reason).await;
              }
              ConnectionAction::Break => break,
            }
          }
          Err(_) => {
            tracing::error!("failed to setup connection attempt, exiting");
            break;
          }
        }
      } else {
        break;
      }

      // Check if we're shutting down before retrying
      if is_shutting_down.load(Ordering::SeqCst) {
        tracing::info!("shutting down, stopping retry loop");
        break;
      }
    }

    // Cancel any remaining ping task when exiting
    if let Some(token) = ping_cancel_token.take() {
      token.cancel();
    }

    tracing::debug!("exiting");
  }

  async fn send_error_notifications(
    id: u64,
    err: Error,
    parent: &Addr<ControllerClient>,
    should_terminate: bool,
  ) {
    let (error_message_text, reject_reason) = match &err {
      Error::ConnectionRequestRejected(reason) => {
        (format!("server rejected: {:?}", reason), reason.clone())
      }
      other => (other.to_string(), RejectReason::Unknown),
    };

    SendWs::new(
      id,
      OutgoingMessage::ConnectRejected(messages::ConnectRejected {
        message: error_message_text,
        reason: reject_reason,
        will_retry: !should_terminate,
      }),
    )
    .notify(parent)
    .await
    .ok();

    parent
      .notify(
        ControllerEventData::ConnectionError(ConnectionError {
          error: err,
          should_terminate,
        })
        .wrap(id),
      )
      .await
      .ok();
  }

  async fn send_disconnect_notifications(
    id: u64,
    parent: &Addr<ControllerClient>,
    should_terminate: bool,
  ) {
    // Send disconnect message to frontend
    SendWs::new(
      id,
      OutgoingMessage::Disconnect(messages::Disconnect {
        reason: messages::DisconnectReason::Unknown,
        message: "Connection failed".to_string(),
        will_retry: !should_terminate,
      }),
    )
    .notify(parent)
    .await
    .ok();

    // Send disconnect event to parent controller
    parent
      .notify(ControllerEventData::Disconnected(Disconnected { should_terminate }).wrap(id))
      .await
      .ok();
  }

  async fn setup_connection_attempt(
    ping_cancel_token: &mut Option<CancellationToken>,
    id: u64,
    owner: &Addr<Self>,
    parent: &Addr<ControllerClient>,
    nodes: &Addr<NodeRegistry>,
  ) -> Option<Result<Receiver<Frame>, ()>> {
    // Cancel previous ping task if it exists
    if let Some(token) = ping_cancel_token.take() {
      tracing::debug!("cancelling previous ping task");
      token.cancel();
    }

    // Create new channel and ping task for this connection attempt
    let (new_tx, new_rx) = channel(5);

    // Update the owner's frame_tx for this attempt
    if let Err(_) = owner
      .send(UpdateFrameTx {
        frame_tx: new_tx.clone(),
      })
      .await
    {
      tracing::error!("failed to update frame_tx, owner may be gone");
      return None;
    }

    // Start new ping task for this connection attempt
    *ping_cancel_token = Some(Self::spawn_ping_task(id, new_tx, parent, nodes));
    tracing::debug!("started new ping task for connection attempt");

    Some(Ok(new_rx))
  }

  async fn execute_connection_attempt(
    id: u64,
    domain: &str,
    token: &str,
    frame_rx: Receiver<Frame>,
    owner: &Addr<Self>,
    parent: &Addr<ControllerClient>,
    nodes: &Addr<NodeRegistry>,
    iteration_count: u32,
    retry_count: u32,
  ) -> ConnectionResult {
    let start_time = Instant::now();

    let result = Self::connect_and_serve(
      id,
      domain,
      token.to_string(),
      frame_rx,
      owner.clone(),
      parent.clone(),
      nodes.clone(),
    )
    .instrument(tracing::info_span!(
      "connect_and_serve",
      iteration = iteration_count,
      retry = retry_count
    ))
    .await;

    ConnectionResult {
      result,
      duration: start_time.elapsed(),
    }
  }

  async fn handle_connection_outcome(
    outcome: ConnectionResult,
    retry_count: &mut u32,
    parent: &Addr<ControllerClient>,
    is_shutting_down: &Arc<AtomicBool>,
    id: u64,
  ) -> ConnectionAction {
    const RESET_THRESHOLD: Duration = Duration::from_secs(60);

    match outcome.result {
      Ok(_) => {
        // When the handler disconnected without an error, it means we should terminate
        tracing::info!(
          "connection ended with explicit disconnect after {:?}, terminating",
          outcome.duration
        );
        // Send disconnect notifications for graceful termination
        Self::send_disconnect_notifications(id, parent, true).await;
        ConnectionAction::Break
      }
      Err(err) => {
        // Check termination conditions
        if is_shutting_down.load(Ordering::SeqCst) && matches!(err, Error::TaskCancelled(_)) {
          tracing::debug!("controller stream cancelled during shutdown: {}", err);
          // Only send disconnect notifications for explicit shutdowns
          Self::send_disconnect_notifications(id, parent, true).await;
          return ConnectionAction::Break;
        }

        // Check for non-recoverable rejection reasons
        if let Error::ConnectionRequestRejected(reason) = &err {
          if !matches!(reason, RejectReason::Unknown) {
            tracing::error!(
              "connection rejected with non-recoverable reason: {:?}",
              reason
            );
            Self::send_error_notifications(id, err, parent, true).await;
            // Only send disconnect notifications for explicit rejections
            Self::send_disconnect_notifications(id, parent, true).await;
            return ConnectionAction::Break;
          }
        }

        // Reset retry count if connection lasted more than 60 seconds before failing
        if outcome.duration >= RESET_THRESHOLD {
          *retry_count = 0;
          tracing::debug!(
            "connection lasted {:?} before failing, resetting retry count",
            outcome.duration
          );
        }

        let error_string = err.to_string();
        tracing::error!("controller stream error: {}", err);
        // Send error notifications for all errors that reach here
        Self::send_error_notifications(id, err, parent, false).await;
        ConnectionAction::Continue(error_string)
      }
    }
  }

  async fn wait_for_retry(
    id: u64,
    retry_count: &mut u32,
    parent: &Addr<ControllerClient>,
    reason: String,
  ) {
    const MIN_DELAY: Duration = Duration::from_secs(5);
    const MAX_DELAY: Duration = Duration::from_secs(30);
    const EXPONENT: f64 = 1.3;
    const MAX_RETRY_COUNT: u32 = 20; // Cap to prevent overflow

    // Calculate exponential backoff delay with capped retry count
    let capped_retry_count = (*retry_count).min(MAX_RETRY_COUNT);
    let delay_secs = MIN_DELAY.as_secs_f64() * EXPONENT.powi(capped_retry_count as i32);
    let delay = Duration::from_secs_f64(delay_secs).min(MAX_DELAY);
    *retry_count += 1;

    tracing::info!(
      "retrying connection in {:?} (attempt #{}) - reason: {}",
      delay,
      *retry_count,
      reason
    );

    // Send retry notification to the upstream application
    if let Err(_) = parent
      .notify(SendWs::new(
        id,
        OutgoingMessage::ConnectRetrying(ConnectRetrying {
          attempt: *retry_count,
          delay_secs: delay.as_secs(),
          reason,
        }),
      ))
      .await
    {
      tracing::warn!("failed to send ConnectRetrying message");
    }

    sleep(delay).await;
  }

  fn spawn_ping_task(
    id: u64,
    frame_tx: Sender<Frame>,
    parent: &Addr<ControllerClient>,
    nodes: &Addr<NodeRegistry>,
  ) -> CancellationToken {
    let ping_token = CancellationToken::new();
    tokio::spawn({
      let frame_tx = frame_tx;
      let parent = parent.clone();
      let nodes = nodes.clone();
      let token = ping_token.clone();
      async move {
        // Initial delay before starting ping reports
        sleep(Duration::from_secs(2)).await;
        loop {
          tokio::select! {
            _ = token.cancelled() => {
              tracing::debug!("ping task cancelled");
              break;
            }
            _ = sleep(Duration::from_secs(5)) => {
              // Check cancellation before attempting to report ping
              if token.is_cancelled() {
                tracing::debug!("ping task cancelled before ping report");
                break;
              }
              tokio::select! {
                _ = token.cancelled() => {
                  tracing::debug!("ping task cancelled during ping report");
                  break;
                }
                result = Self::report_ping(id, frame_tx.clone(), &parent, &nodes) => {
                  if let Err(err) = result {
                    // Don't log as error if it's just a cancelled task - this is expected during connection retries
                    if matches!(err, Error::TaskCancelled(_)) {
                      tracing::debug!("ping report failed due to cancelled task: {}", err);
                      // If the frame channel is closed, we should exit the ping task
                      if frame_tx.is_closed() {
                        tracing::debug!("frame channel closed, stopping ping task");
                        break;
                      }
                    } else {
                      tracing::error!("report ping: {}", err);
                    }
                  }
                }
              }
            }
          }
        }
      }
      .instrument(tracing::debug_span!("ping_task", worker_id = id))
    });
    ping_token
  }

  async fn report_ping(
    id: u64,
    frame_tx: Sender<Frame>,
    parent: &Addr<ControllerClient>,
    nodes: &Addr<NodeRegistry>,
  ) -> Result<()> {
    // Check if the channel is closed before doing any work
    if frame_tx.is_closed() {
      return Err(Error::TaskCancelled(anyhow::format_err!(
        "frame channel is closed"
      )));
    }

    let ping_map = nodes.send(GetNodePingMap).await??;
    parent
      .notify(SendWs::new(
        id,
        OutgoingMessage::PingUpdate(PingUpdate {
          ping_map: ping_map.clone(),
        }),
      ))
      .await?;

    // Check again before sending to frame_tx
    if frame_tx.is_closed() {
      return Err(Error::TaskCancelled(anyhow::format_err!(
        "frame channel is closed"
      )));
    }

    frame_tx
      .send(
        proto::PacketPlayerPingMapUpdateRequest {
          ping_map: ping_map
            .into_iter()
            .map(|(k, v)| Ok((k, v.pack()?)))
            .collect::<Result<Vec<(i32, proto::PingStats)>>>()?
            .into_iter()
            .collect(),
        }
        .encode_as_frame()?,
      )
      .await
      .map_err(|_| Error::TaskCancelled(anyhow::format_err!("controller stream worker gone")))?;
    Ok(())
  }

  async fn connect_and_serve(
    id: u64,
    domain: &str,
    token: String,
    mut frame_receiver: Receiver<Frame>,
    owner: Addr<Self>,
    parent: Addr<ControllerClient>,
    nodes_reg: Addr<NodeRegistry>,
  ) -> Result<()> {
    let addr = format!("{}:{}", domain, flo_constants::CONTROLLER_SOCKET_PORT);
    tracing::debug!("connect addr: {}", addr);

    let mut stream = FloStream::connect_no_delay(addr).await?;

    tracing::debug!("connected");

    stream
      .send(proto::PacketClientConnect {
        connect_version: Some(crate::version::FLO_VERSION.into()),
        token,
      })
      .await?;

    let reply = stream.recv_frame().await?;

    let (session, nodes): (PlayerSession, _) = flo_net::try_flo_packet! {
      reply => {
        p: proto::PacketClientConnectAccept => {
          (
            PlayerSession::unpack(p.session)?,
            p.nodes
          )
        }
        p: proto::PacketClientConnectReject => {
          return Err(Error::ConnectionRequestRejected(S2ProtoEnum::unpack_enum(p.reason())))
        }
      }
    };

    let player_id = session.player.id;

    tracing::debug!(
      player_id,
      "player = {}, status = {:?}",
      session.player.id,
      session.status
    );

    parent
      .notify(ControllerEventData::Connected.wrap(id))
      .await?;
    parent
      .notify(
        ControllerEventData::PlayerSessionUpdate(PlayerSessionUpdateEvent::Full(session.clone()))
          .wrap(id),
      )
      .await?;
    parent.send(UpdateNodes { nodes }).await??;
    parent
      .notify(SendWs::new(
        id,
        messages::OutgoingMessage::PlayerSession(session),
      ))
      .await?;

    loop {
      tokio::select! {
        next_send = frame_receiver.recv() => {
          if let Some(frame) = next_send {
            match stream.send_frame_timeout(frame).await {
              Ok(_) => {},
              Err(e) => {
                tracing::debug!("exiting: send error: {}", e);
                return Err(Error::ControllerDisconnected(e.into()));
              }
            }
          } else {
            tracing::debug!("exiting: sender dropped");
            break;
          }
        }
        recv = stream.recv_frame() => {
          match recv {
            Ok(mut frame) => {
              if frame.type_id == PacketTypeId::Ping {
                frame.type_id = PacketTypeId::Pong;
                match stream.send_frame_timeout(frame).await {
                  Ok(_) => {
                    continue;
                  },
                  Err(e) => {
                    tracing::debug!("exiting: send error: {}", e);
                    return Err(Error::ControllerDisconnected(e.into()));
                  }
                }
              }

              let type_id = frame.type_id;
              match Self::handle_frame(id, player_id, frame, &mut stream, &owner, &parent, &nodes_reg).await {
                Ok(_) => {},
                Err(e) => {
                  tracing::error!("handle frame: {}", e);
                }
              }

              if type_id == PacketTypeId::LobbyDisconnect {
                tracing::info!("lobby disconnect received");
                // TODO: This packet is currently never thrown, but if it does,
                // we might want to have special handling to not consider it an error.
                break;
              }
            },
            Err(e) => {
              tracing::debug!("exiting: recv: {}", e);
              return Err(Error::ControllerDisconnected(e.into()));
            }
          }
        }
      }
    }

    tracing::debug!("exiting");
    // Getting to this point means we want to actively terminate without retries.
    Ok(())
  }

  // handle controller packets
  async fn handle_frame(
    id: u64,
    player_id: i32,
    frame: Frame,
    stream: &mut FloStream,
    owner: &Addr<Self>,
    parent: &Addr<ControllerClient>,
    nodes: &Addr<NodeRegistry>,
  ) -> Result<()> {
    flo_net::try_flo_packet! {
      frame => {
        p: proto::PacketClientDisconnect => {
          SendWs::new(id, OutgoingMessage::Disconnect(messages::Disconnect {
              reason: S2ProtoEnum::unpack_i32(p.reason)?,
              message: format!("Server closed the connection: {:?}", p.reason),
              will_retry: false,
            })).notify(parent).await?;
        }
        p: proto::PacketGameInfo => {
          parent.notify(ControllerEventData::SelectNode(p.game.as_ref().and_then(|g| {
            g.node.as_ref().map(|node| node.id)
          })).wrap(id)).await?;

          let game = GameInfo::unpack(p.game)?;

          let local_game_info = Arc::new(local_game_from_game_info(player_id, &game)?);
          owner.send(SetLocalGameInfo(local_game_info.clone().into())).await??;

          SendWs::new(id, OutgoingMessage::CurrentGameInfo(game)).notify(parent).await?;
        }
        p: proto::PacketGamePlayerEnter => {
          let slot_index = p.slot_index;
          let slot = p.slot.clone();
          owner.send(UpdateLocalGameInfo::new(
            move |info| -> Result<_> {
              if let Some(r) = info.slots.get_mut(slot_index as usize) {
                *r = Slot::unpack(slot)?;
                Ok(())
              } else {
                tracing::error!("PacketGamePlayerEnter: invalid slot index: {}", slot_index);
                Err(Error::InvalidMapInfo)
              }
            }
          )).await??;
          SendWs::new(
            id,
            OutgoingMessage::GamePlayerEnter(S2ProtoUnpack::unpack(p)?)
          ).notify(parent).await?;
        }
        p: proto::PacketGamePlayerLeave => {
          owner.send(UpdateLocalGameInfo::new({
            let p = p.clone();
            move |info| -> Result<_> {
              if let Some(slot) = info.slots.iter_mut().find(|s| s.player.as_ref().map(|p| p.id) == Some(p.player_id)) {
                *slot = Slot::default();
                Ok(())
              } else {
                tracing::error!(game_id = p.game_id, player_id = p.player_id, "PacketGamePlayerLeave: player slot not found");
                Err(Error::InvalidMapInfo)
              }
            }
          })).await??;
          SendWs::new(
            id,
            OutgoingMessage::GamePlayerLeave(p)
          ).notify(parent).await?;
        }
        p: proto::PacketGameSlotUpdate => {
          owner.send(UpdateLocalGameInfo::new({
            let p = p.clone();
            move |info| -> Result<_> {
              if let Some(slot) = info.slots.get_mut(p.slot_index as usize) {
                slot.player = p.player.map(PlayerInfo::unpack).transpose()?;
                slot.settings = SlotSettings::unpack(p.slot_settings.clone())?;
                Ok(())
              } else {
                tracing::error!("PacketGamePlayerEnter: invalid slot index: {}", p.slot_index);
                Err(Error::InvalidMapInfo)
              }
            }
          })).await??;
          SendWs::new(
            id,
            OutgoingMessage::GameSlotUpdate(S2ProtoUnpack::unpack(p)?)
          ).notify(parent).await?;
        }
        p: proto::PacketPlayerSessionUpdate => {
          let session = PlayerSessionUpdate::unpack(p)?;
          parent.notify(ControllerEventData::PlayerSessionUpdate(PlayerSessionUpdateEvent::Partial(session.clone())).wrap(id)).await?;
          if session.game_id.is_none() {
            owner.send(SetLocalGameInfo(None)).await??;
          }
          SendWs::new(
            id,
            OutgoingMessage::PlayerSessionUpdate(session)
          ).notify(parent).await?;
        }
        p: proto::PacketListNodes => {
          parent
            .send(UpdateNodes{ nodes: p.nodes.clone() })
            .await??;
        }
        p: proto::PacketGameSelectNode => {
          parent.notify(ControllerEventData::SelectNode(p.node_id).wrap(id)).await?;
          owner.send(UpdateLocalGameInfo::new({
            let node_id = p.node_id;
            move |info| -> Result<_> {
              info.node_id = node_id;
              Ok(())
            }
          })).await??;
          SendWs::new(
            id,
            OutgoingMessage::GameSelectNode(p)
          ).notify(parent).await?;
        }
        p: proto::PacketPlayerPingMapUpdate => {
          SendWs::new(
            id,
            OutgoingMessage::PlayerPingMapUpdate(p)
          ).notify(parent).await?;
        }
        p: proto::PacketGamePlayerPingMapSnapshot => {
          SendWs::new(
            id,
            OutgoingMessage::GamePlayerPingMapSnapshot(p)
          ).notify(parent).await?;
        }
        p: proto::PacketGameStartReject => {
          SendWs::new(
            id,
            OutgoingMessage::GameStartReject(p)
          ).notify(parent).await?;
        }
        p: proto::PacketGameStarting => {
          let info = owner.send(GetGameStartClientInfo {
            game_id: p.game_id
          }).await??;
          if let Some(info) = info {
            stream.send(flo_net::proto::flo_connect::PacketGameStartPlayerClientInfoRequest {
              game_id: p.game_id,
              war3_version: info.war3_version,
              map_sha1: info.map_sha1,
            }).await?;
            SendWs::new(
              id,
              OutgoingMessage::GameStarting(p)
            ).notify(parent).await?;
          }
        }
        p: proto::PacketGamePlayerToken => {
          let info = owner.send(GetLocalGameInfo).await?;
          if let Some(info) = info {
            if info.game_id == p.game_id {
              parent.notify(ControllerEventData::GameReceived(GameReceivedEvent {
                node_id: p.node_id,
                game_info: info,
                player_token: p.player_token,
              }).wrap(id)).await?;
            } else {
              tracing::warn!("received player for game#{} but the active game id is {}", p.game_id, info.game_id);
            }
          } else {
            tracing::warn!("received player token but there is no active game");
          }
        }
        p: proto::PacketPlayerMuteListUpdate => {
          tracing::debug!("mute list update: {:?}", p.mute_list);
          parent.notify(UpdateMuteList {
            mute_list: p.mute_list
          }).await?;
        }
        // client status update from node
        p: proto::PacketGameSlotClientStatusUpdate => {
          SendWs::new(
            id,
            OutgoingMessage::GameSlotClientStatusUpdate(S2ProtoUnpack::unpack(p)?)
          ).notify(parent).await?;
        }
        p: flo_net::proto::flo_node::PacketNodeGameStatusUpdate => {
          SendWs::new(
            id,
            OutgoingMessage::GameStatusUpdate(p.into())
          ).notify(parent).await?;
        }
        p: proto::PacketAddNode => {
          nodes
            .notify(AddNode { node: p.node.extract()? })
            .await?;
        }
        p: proto::PacketRemoveNode => {
          nodes
            .notify(RemoveNode { node_id: p.node_id })
            .await?;
        }
      }
    };
    Ok(())
  }
}

impl Drop for ControllerStream {
  fn drop(&mut self) {
    self.is_shutting_down.store(true, Ordering::SeqCst);
    tracing::info!("ControllerStream dropped");
  }
}

#[async_trait]
impl Actor for ControllerStream {
  async fn started(&mut self, ctx: &mut Context<Self>) {
    let _frame_rx = if let Some(rx) = self.frame_rx.take() {
      rx
    } else {
      let (frame_tx, frame_rx) = channel(5);
      self.frame_tx = frame_tx;
      frame_rx
    };

    ctx.spawn(
      Self::run_connection_worker(
        self.id,
        self.domain.clone(),
        self.token.clone(),
        ctx.addr(),
        self.parent.clone(),
        self.nodes.clone(),
        self.is_shutting_down.clone(),
      )
      .instrument(tracing::debug_span!("worker", id = self.id)),
    );
  }
}

struct UpdateLocalGameInfo<F>
where
  F: FnOnce(&mut LocalGameInfo) -> Result<()>,
{
  f: F,
}

impl<F> UpdateLocalGameInfo<F>
where
  F: FnOnce(&mut LocalGameInfo) -> Result<()> + Send + 'static,
{
  fn new(f: F) -> Self {
    Self { f }
  }
}

impl<F> Message for UpdateLocalGameInfo<F>
where
  F: FnOnce(&mut LocalGameInfo) -> Result<()> + Send + 'static,
{
  type Result = Result<Arc<LocalGameInfo>>;
}

#[async_trait]
impl<F> Handler<UpdateLocalGameInfo<F>> for ControllerStream
where
  F: FnOnce(&mut LocalGameInfo) -> Result<()> + Send + 'static,
{
  async fn handle(
    &mut self,
    _: &mut Context<Self>,
    UpdateLocalGameInfo { f }: UpdateLocalGameInfo<F>,
  ) -> <UpdateLocalGameInfo<F> as Message>::Result {
    let r = self
      .current_game_info
      .clone()
      .ok_or_else(|| Error::LocalGameInfoNotFound)?;
    let mut mutated = r.clone();

    f(Arc::make_mut(&mut mutated))?;
    if !Arc::ptr_eq(&r, &mutated) {
      self.current_game_info.replace(mutated.clone());
    }

    self
      .parent
      .notify(
        ControllerEventData::GameInfoUpdate(GameInfoUpdateEvent {
          game_info: Some(mutated.clone()),
        })
        .wrap(self.id),
      )
      .await?;

    Ok(mutated)
  }
}

struct SetLocalGameInfo(Option<Arc<LocalGameInfo>>);

impl Message for SetLocalGameInfo {
  type Result = Result<()>;
}

#[async_trait]
impl Handler<SetLocalGameInfo> for ControllerStream {
  async fn handle(
    &mut self,
    _: &mut Context<Self>,
    SetLocalGameInfo(info): SetLocalGameInfo,
  ) -> <SetLocalGameInfo as Message>::Result {
    if let Some(info) = info {
      self
        .parent
        .notify(ControllerEventData::SelectNode(info.node_id.clone()).wrap(self.id))
        .await?;
      self
        .parent
        .notify(
          ControllerEventData::GameInfoUpdate(GameInfoUpdateEvent {
            game_info: Some(info.clone()),
          })
          .wrap(self.id),
        )
        .await?;
      self.current_game_info.replace(info);
    } else {
      self.current_game_info.take();
      self
        .parent
        .notify(ControllerEventData::SelectNode(None).wrap(self.id))
        .await?;
      self
        .parent
        .notify(
          ControllerEventData::GameInfoUpdate(GameInfoUpdateEvent { game_info: None })
            .wrap(self.id),
        )
        .await?;
    }
    Ok(())
  }
}

struct GetLocalGameInfo;

impl Message for GetLocalGameInfo {
  type Result = Option<Arc<LocalGameInfo>>;
}

#[async_trait]
impl Handler<GetLocalGameInfo> for ControllerStream {
  async fn handle(
    &mut self,
    _: &mut Context<Self>,
    _: GetLocalGameInfo,
  ) -> <GetLocalGameInfo as Message>::Result {
    self.current_game_info.clone()
  }
}

struct GetGameStartClientInfo {
  game_id: i32,
}

struct GameStartClientInfo {
  war3_version: String,
  map_sha1: Vec<u8>,
}

impl Message for GetGameStartClientInfo {
  type Result = Result<Option<GameStartClientInfo>>;
}

#[async_trait]
impl Handler<GetGameStartClientInfo> for ControllerStream {
  async fn handle(
    &mut self,
    _: &mut Context<Self>,
    GetGameStartClientInfo { game_id }: GetGameStartClientInfo,
  ) -> <GetGameStartClientInfo as Message>::Result {
    if let Some(info) = self.current_game_info.as_ref() {
      if info.game_id == game_id {
        let client_info = self
          .platform
          .send(GetClientPlatformInfo { force_reload: true })
          .await?
          .map_err(|_| Error::War3NotLocated)?;
        let war3_version = client_info.version;
        let map_sha1 = self
          .platform
          .send(CalcMapChecksum {
            path: info.map_path.clone(),
          })
          .await??
          .sha1
          .to_vec();
        return Ok(Some(GameStartClientInfo {
          war3_version,
          map_sha1,
        }));
      }
    }
    Ok(None)
  }
}

pub struct SendFrame(pub Frame);

impl Message for SendFrame {
  type Result = Result<()>;
}

#[async_trait]
impl Handler<SendFrame> for ControllerStream {
  async fn handle(
    &mut self,
    _: &mut Context<Self>,
    SendFrame(frame): SendFrame,
  ) -> <SendFrame as Message>::Result {
    match self.frame_tx.send(frame).await {
      Ok(_) => Ok(()),
      Err(_) => Err(Error::TaskCancelled(anyhow::format_err!(
        "controller stream worker gone"
      ))),
    }
  }
}

struct UpdateFrameTx {
  frame_tx: Sender<Frame>,
}

impl Message for UpdateFrameTx {
  type Result = Result<()>;
}

#[async_trait]
impl Handler<UpdateFrameTx> for ControllerStream {
  async fn handle(
    &mut self,
    _: &mut Context<Self>,
    UpdateFrameTx { frame_tx }: UpdateFrameTx,
  ) -> <UpdateFrameTx as Message>::Result {
    self.frame_tx = frame_tx;
    tracing::debug!("updated frame_tx for new connection attempt");
    Ok(())
  }
}

#[derive(Debug)]
pub struct ControllerEvent {
  pub id: u64,
  pub data: ControllerEventData,
}

impl Message for ControllerEvent {
  type Result = ();
}

#[derive(Debug)]
pub enum ControllerEventData {
  Connected,
  ConnectionError(ConnectionError),
  PlayerSessionUpdate(PlayerSessionUpdateEvent),
  GameInfoUpdate(GameInfoUpdateEvent),
  GameReceived(GameReceivedEvent),
  SelectNode(Option<i32>),
  Disconnected(Disconnected),
}

impl ControllerEventData {
  fn wrap(self, id: u64) -> ControllerEvent {
    ControllerEvent { id, data: self }
  }
}

#[derive(Debug)]
pub struct Disconnected {
  pub should_terminate: bool,
}

#[derive(Debug)]
pub struct ConnectionError {
  pub error: Error,
  pub should_terminate: bool,
}

#[derive(Debug)]
pub enum PlayerSessionUpdateEvent {
  Full(PlayerSession),
  Partial(PlayerSessionUpdate),
}

#[derive(Debug)]
pub struct GameInfoUpdateEvent {
  pub game_info: Option<Arc<LocalGameInfo>>,
}

#[derive(Debug)]
pub struct GameReceivedEvent {
  pub node_id: i32,
  pub game_info: Arc<LocalGameInfo>,
  pub player_token: Vec<u8>,
}

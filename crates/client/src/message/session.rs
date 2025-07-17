use super::messages::{
  ClientInfo, ErrorMessage, IncomingMessage, MapList, MapPath, OutgoingMessage, War3Info,
  WatchGameInfo,
};
use super::{ConnectController, MessageEvent};
use crate::controller::{
  ClearNodeAddrOverrides, ControllerClient, SendFrame, SetNodeAddrOverrides,
};
use crate::error::{Error, Result};
use crate::message::stream::MessageStream;
use crate::messages::GenericFloErrorCode;
use crate::messages::GenericFloError;
use crate::observer::{ObserverClient, ObserverHostShared};
use crate::platform::{
  GetClientPlatformInfo, GetMapDetail, GetMapList, KillTestGame, Platform, PlatformStateError,
  Reload,
};
use flo_net::packet::FloPacket;
use flo_net::proto::flo_connect::{
  PacketGamePlayerPingMapSnapshotRequest, PacketGameSlotUpdateRequest, PacketGameStartRequest,
  PacketListNodesRequest,
};
use flo_platform::ClientPlatformInfo;
use flo_state::Addr;
use flo_task::{SpawnScope, SpawnScopeHandle};
use parking_lot::Mutex;
use s2_grpc_utils::S2ProtoPack;
use std::sync::Arc;
use tokio::sync::mpsc::{channel, Receiver, Sender, WeakSender};
use tokio::sync::oneshot;
use tracing_futures::Instrument;
use std::error::Error as StdError;

#[derive(Debug)]
pub struct Session {
  _scope: SpawnScope,
  tx: Sender<OutgoingMessage>,
}

impl Session {
  pub fn new(
    platform: Addr<Platform>,
    controller_client: Addr<ControllerClient>,
    observer_client: Addr<ObserverClient>,
    stream: Box<dyn MessageStream>,
  ) -> Self {
    let (tx, rx) = channel(3);
    let scope = SpawnScope::new();
    let serve_state = Arc::new(Worker {
      platform,
      controller_client,
      observer_client,
      current_observer_host: Mutex::new(None),
      pending_password_request: Arc::new(Mutex::new(None)),
    });
    tokio::spawn(
      {
        let scope = scope.handle();
        async move {
          if let Err(e) = serve_stream(serve_state.clone(), rx, scope, stream).await {
            tracing::error!("serve stream: {}", e);
          } else {
            tracing::debug!("exiting");
          }
        }
      }
      .instrument(tracing::debug_span!("worker")),
    );
    Self { _scope: scope, tx }
  }

  pub fn sender(&self) -> MessageSender {
    MessageSender(self.tx.clone())
  }
}

#[derive(Debug, Clone)]
pub struct MessageSender(Sender<OutgoingMessage>);

impl MessageSender {
  pub async fn send_or_discard(&self, msg: OutgoingMessage) {
    self.0.clone().send(msg).await.ok();
  }

  pub fn downgrade(&self) -> WeakSender<OutgoingMessage> {
    self.0.downgrade()
  }
}

async fn serve_stream(
  state: Arc<Worker>,
  mut rx: Receiver<OutgoingMessage>,
  mut scope: SpawnScopeHandle,
  mut stream: Box<dyn MessageStream>,
) -> Result<()> {
  let msg = state.get_client_info_message().await?;

  stream.send(msg).await?;

  let (reply_sender, mut receiver) = channel(3);

  loop {
    tokio::select! {
      _ = scope.left() => {
        rx.close();
        while let Some(msg) = rx.recv().await {
          stream.send(msg).await?;
        }
        stream.flush().await;
        break;
      }
      msg = rx.recv() => {
        if let Some(msg) = msg {
          stream.send(msg).await?;
        } else {
          tracing::debug!("message sender dropped");
          break;
        }
      }
      msg = receiver.recv() => {
        let msg = msg.expect("sender hold on stack");
        stream.send(msg).await?;
      }
      next = stream.recv() => {
        let msg = if let Some(msg) = next {
          msg
        } else {
          tracing::debug!("stream closed");
          break;
        };

        state.handle_message(&reply_sender, msg).await?;
      }
    }
  }

  Ok(())
}

struct Worker {
  platform: Addr<Platform>,
  controller_client: Addr<ControllerClient>,
  observer_client: Addr<ObserverClient>,
  current_observer_host: Mutex<Option<ObserverHostShared>>,
  pending_password_request: Arc<Mutex<Option<oneshot::Sender<String>>>>,
}

impl Worker {
  async fn handle_message(
    &self,
    reply_sender: &Sender<OutgoingMessage>,
    msg: IncomingMessage,
  ) -> Result<()> {
    match msg {
      IncomingMessage::ReloadClientInfo => {
        self.handle_reload_client_info(reply_sender.clone()).await?;
      }
      IncomingMessage::Connect(connect) => {
        self.handle_connect(connect.token).await?;
      }
      IncomingMessage::Disconnect => {
        self.handle_disconnect().await?;
      }
      IncomingMessage::ListMaps => {
        self.handle_map_list(reply_sender.clone()).await?;
      }
      IncomingMessage::GetMapDetail(payload) => {
        self
          .handle_get_map_detail(reply_sender.clone(), payload)
          .await?;
      }
      IncomingMessage::GameSlotUpdateRequest(req) => {
        self
          .send_frame::<PacketGameSlotUpdateRequest>(req.pack()?)
          .await?;
      }
      IncomingMessage::ListNodesRequest => {
        self.send_frame(PacketListNodesRequest {}).await?;
      }
      IncomingMessage::GameSelectNodeRequest(req) => {
        self.send_frame(req).await?;
      }
      IncomingMessage::GamePlayerPingMapSnapshotRequest(req) => {
        self
          .send_frame::<PacketGamePlayerPingMapSnapshotRequest>(req)
          .await?;
      }
      IncomingMessage::GameStartRequest(req) => {
        self.send_frame::<PacketGameStartRequest>(req).await?;
      }
      IncomingMessage::StartTestGame(msg) => {
        let res = self
            .platform
            .send(crate::platform::StartTestGame {
                name: msg.name,
                outgoing_sender: reply_sender.downgrade(),
            })
            .await
            .map_err(Error::from)
            .and_then(|r| r);
    
        if let Err(err) = res {
            if let Some(flo_w3map::error::Error::Storage(_)) = StdError::source(&err).and_then(|e| e.downcast_ref::<flo_w3map::error::Error>())
            {
                // Special handling for Map storage errors:
                tracing::error!("Error extracting WC3Map: {err}");
                reply_sender
                    .clone()
                    .send(OutgoingMessage::GenericFloError(
                        GenericFloError::new(err, GenericFloErrorCode::GamefilesCorrupted),
                    ))
                    .await?;
            } else {
                // Generic error handling:
                tracing::error!("Unexpected error during StartTestGame: {err}");
                reply_sender
                    .clone()
                    .send(OutgoingMessage::GenericFloError(
                        GenericFloError::new(err, GenericFloErrorCode::Unknown),
                    ))
                    .await?;
            }
        }
    }
    
      IncomingMessage::KillTestGame => {
        self.platform.notify(KillTestGame).await?;
      }
      IncomingMessage::SetNodeAddrOverrides(req) => {
        let res = self
          .controller_client
          .send(SetNodeAddrOverrides {
            overrides: req.overrides,
          })
          .await
          .map_err(Error::from)
          .and_then(|r| r);
        if let Err(err) = res {
          tracing::error!("set node addr override: {}", err);
          reply_sender
            .clone()
            .send(OutgoingMessage::SetNodeAddrOverridesError(
              ErrorMessage::new(err),
            ))
            .await?;
        }
      }
      IncomingMessage::ClearNodeAddrOverrides => {
        self
          .controller_client
          .send(ClearNodeAddrOverrides)
          .await??;
      }
      IncomingMessage::WatchGame(msg) => {
        // Create a password callback that communicates with the upstream application
        let password_callback = self.create_password_callback(reply_sender.clone());
        
        let res = self.observer_client.send(crate::observer::WatchGame {
          token: msg.token,
          password_callback: Box::new(password_callback),
        }).await?;
        match res {
          Ok(shared) => {
            reply_sender
              .send(OutgoingMessage::WatchGame(WatchGameInfo {
                game_id: shared.game_id,
                delay_secs: shared.initial_delay_secs.clone(),
                speed: shared.speed(),
              }))
              .await?;
            self.current_observer_host.lock().replace(shared);
          }
          Err(err) => {
            tracing::error!("watch game: {}", err);
            reply_sender
              .send(OutgoingMessage::WatchGameError(ErrorMessage::new(err)))
              .await?;
          }
        }
      }
      IncomingMessage::WatchGameSetSpeed(msg) => {
        let reply = {
          let host = self.current_observer_host.lock();
          if let Some(host) = host.as_ref() {
            host.set_speed(msg.speed);
            OutgoingMessage::WatchGame(WatchGameInfo {
              game_id: host.game_id,
              delay_secs: host.initial_delay_secs.clone(),
              speed: msg.speed,
            })
          } else {
            OutgoingMessage::WatchGameSetSpeedError(ErrorMessage::new("No active stream."))
          }
        };
        reply_sender.send(reply).await?;
      }
      IncomingMessage::FloTvPasswordSubmit(msg) => {
        let mut pending_request = self.pending_password_request.lock();
        if let Some(sender) = pending_request.take() {
          // Send the password to the waiting observer connection
          if let Err(_) = sender.send(msg.password_sha256) {
            tracing::warn!("Failed to send password to observer connection - receiver may have been dropped");
          }
        } else {
          tracing::warn!("Received FloTvPasswordSubmit but no pending password request");
        }
      }
    }
    Ok(())
  }

  fn create_password_callback(&self, reply_sender: Sender<OutgoingMessage>) -> impl Fn() -> Option<String> {
    let pending_request = self.pending_password_request.clone();
    move || {
      let (tx, rx) = oneshot::channel();
      
      // Store the sender for later use when password is submitted
      {
        let mut pending = pending_request.lock();
        *pending = Some(tx);
      }
      
      // Send password required message to upstream
      let reply_sender = reply_sender.clone();
      tokio::spawn(async move {
        if let Err(err) = reply_sender.send(OutgoingMessage::FloTvPasswordRequired).await {
          tracing::error!("Failed to send FloTvPasswordRequired message: {}", err);
        }
      });
      
      // Wait for password with timeout
      let rt = tokio::runtime::Handle::current();
      rt.block_on(async {
        match tokio::time::timeout(std::time::Duration::from_secs(120), rx).await {
          Ok(Ok(password)) => Some(password),
          Ok(Err(_)) => {
            tracing::warn!("Password request was cancelled");
            None
          }
          Err(_) => {
            tracing::warn!("Password request timed out");
            None
          }
        }
      })
    }
  }

  async fn get_client_info_message(&self) -> Result<OutgoingMessage> {
    let info = self
      .platform
      .send(GetClientPlatformInfo {
        force_reload: false,
      })
      .await?;
    Ok(OutgoingMessage::ClientInfo(ClientInfo {
      version: crate::version::FLO_VERSION_STRING.into(),
      war3_info: get_war3_info(info),
    }))
  }

  async fn handle_connect(&self, token: String) -> Result<()> {
    self
      .controller_client
      .notify(MessageEvent::ConnectController(ConnectController { token }))
      .await?;
    Ok(())
  }

  async fn handle_disconnect(&self) -> Result<()> {
    self
      .controller_client
      .notify(MessageEvent::Disconnect)
      .await?;
    Ok(())
  }

  async fn handle_reload_client_info(&self, sender: Sender<OutgoingMessage>) -> Result<()> {
    match self.platform.send(Reload).await? {
      Ok(_) => sender.send(self.get_client_info_message().await?),
      Err(e) => sender.send(OutgoingMessage::ReloadClientInfoError(ErrorMessage {
        message: e.to_string(),
      })),
    }
    .await?;
    Ok(())
  }

  async fn handle_map_list(&self, sender: Sender<OutgoingMessage>) -> Result<()> {
    let value = self.platform.send(GetMapList).await?;
    match value {
      Ok(value) => {
        sender
          .send(OutgoingMessage::ListMaps(MapList { data: value }))
          .await?
      }
      Err(e) => {
        sender
          .send(OutgoingMessage::ListMapsError(ErrorMessage::new(e)))
          .await?
      }
    }
    Ok(())
  }

  async fn handle_get_map_detail(
    &self,
    sender: Sender<OutgoingMessage>,
    MapPath { path }: MapPath,
  ) -> Result<()> {
    let detail = self.platform.send(GetMapDetail { path }).await?;
    match detail {
      Ok(detail) => sender.send(OutgoingMessage::GetMapDetail(detail)).await?,
      Err(e) => {
        sender
          .send(OutgoingMessage::GetMapDetailError(ErrorMessage::new(e)))
          .await?;
      }
    }
    Ok(())
  }

  async fn send_frame<T: FloPacket>(&self, pkt: T) -> Result<()> {
    self
      .controller_client
      .send(SendFrame(pkt.encode_as_frame()?))
      .await??;
    Ok(())
  }
}

fn get_war3_info(info: Result<ClientPlatformInfo, PlatformStateError>) -> War3Info {
  match info {
    Ok(info) => War3Info {
      located: true,
      version: info.version.clone().into(),
      error: None,
      user_data_path: info.user_data_path.to_string_lossy().into_owned().into(),
      installation_path: info.installation_path.to_string_lossy().into_owned().into(),
      executable_path: info.executable_path.to_string_lossy().into_owned().into(),
      ptr: info.ptr,
    },
    Err(e) => War3Info {
      located: false,
      version: None,
      error: Some(e),
      user_data_path: None,
      installation_path: None,
      executable_path: None,
      ptr: false,
    },
  }
}

impl From<tokio::sync::mpsc::error::SendError<OutgoingMessage>> for Error {
  fn from(_: tokio::sync::mpsc::error::SendError<OutgoingMessage>) -> Error {
    Error::TaskCancelled(anyhow::format_err!("websocket message dropped"))
  }
}

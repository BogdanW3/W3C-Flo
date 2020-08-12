use futures::stream::StreamExt;
use parking_lot::RwLock;
use s2_grpc_utils::{S2ProtoEnum, S2ProtoUnpack};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing_futures::Instrument;

pub use flo_net::connect::*;
use flo_net::packet::*;
use flo_net::stream::FloStream;

use crate::error::{Error, Result};
use crate::net::node::NodeRegistryRef;
use crate::ws::{message, OutgoingMessage, WsSenderRef};

pub type LobbyStreamSender = mpsc::Sender<Frame>;

#[derive(Debug)]
pub struct LobbyStream {
  frame_sender: mpsc::Sender<Frame>,
  ws_sender: WsSenderRef,
  current_game_id: Arc<RwLock<Option<i32>>>,
}

impl LobbyStream {
  pub async fn connect(
    domain: &str,
    ws_sender: WsSenderRef,
    nodes: NodeRegistryRef,
    token: String,
  ) -> Result<Self> {
    let addr = format!("{}:{}", domain, flo_constants::LOBBY_SOCKET_PORT);

    tracing::debug!("connect addr: {}", addr);

    let mut stream = FloStream::connect(addr).await?;

    stream
      .send(PacketConnectLobby {
        connect_version: Some(crate::version::FLO_VERSION.into()),
        token,
      })
      .await?;

    let reply = stream.recv_frame().await?;

    let session = flo_net::match_packet! {
      reply => {
        p = PacketConnectLobbyAccept => {
          PlayerSession::unpack(p.session)?
        }
        p = PacketConnectLobbyReject => {
          return Err(Error::ConnectionRequestRejected(RejectReason::unpack(p.reason)?))
        }
      }
    };

    let current_game_id = Arc::new(RwLock::new(session.game_id.clone()));

    let (frame_sender, mut frame_r) = mpsc::channel(5);

    Self::send_message(&ws_sender, OutgoingMessage::PlayerSession(session)).await?;

    tokio::spawn({
      let ws_sender = ws_sender.clone();
      let current_game_id = current_game_id.clone();
      async move {
        loop {
          tokio::select! {
            next_send = frame_r.next() => {
              if let Some(frame) = next_send {
                match stream.send_frame(frame).await {
                  Ok(_) => {},
                  Err(e) => {
                    tracing::debug!("exiting: send error: {}", e);
                    break;
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
                    match stream.send_frame(frame).await {
                      Ok(_) => {
                        continue;
                      },
                      Err(e) => {
                        tracing::debug!("exiting: send error: {}", e);
                        break;
                      }
                    }
                  }

                  match Self::dispatch(&ws_sender, &nodes, current_game_id.clone(), frame).await {
                    Ok(_) => {},
                    Err(e) => {
                      tracing::debug!("exiting: dispatch: {}", e);
                      let r =  Self::send_message(&ws_sender, OutgoingMessage::Disconnect(message::Disconnect {
                        reason: DisconnectReason::Unknown,
                        message: format!("dispatch: {}", e)
                      })).await;
                      match r {
                        Ok(_) => {},
                        Err(e) => {
                          tracing::debug!("exiting: send disconnect: {}", e);
                        }
                      }
                      break;
                    }
                  }
                },
                Err(e) => {
                  tracing::debug!("exiting: recv: {}", e);
                  match Self::send_message(&ws_sender, OutgoingMessage::Disconnect(message::Disconnect {
                    reason: DisconnectReason::Unknown,
                    message: format!("recv: {}", e),
                  })).await {
                    Ok(_) => {},
                    Err(e) => {
                      tracing::debug!("exiting: send disconnect: {}", e);
                    }
                  }
                  break;
                }
              }
            }
          }
        }
        tracing::debug!("dropped")
      }
      .instrument(tracing::debug_span!("worker"))
    });

    Ok(LobbyStream {
      frame_sender,
      ws_sender,
      current_game_id,
    })
  }

  pub fn get_sender_cloned(&self) -> mpsc::Sender<Frame> {
    self.frame_sender.clone()
  }

  pub fn current_game_id(&self) -> Option<i32> {
    self.current_game_id.read().clone()
  }

  // forward server packets to the websocket connection
  async fn dispatch(
    sender: &WsSenderRef,
    nodes: &NodeRegistryRef,
    current_game_id: Arc<RwLock<Option<i32>>>,
    frame: Frame,
  ) -> Result<()> {
    let msg = flo_net::match_packet! {
      frame => {
        p = PacketLobbyDisconnect => {
          OutgoingMessage::Disconnect(message::Disconnect {
            reason: S2ProtoEnum::unpack_i32(p.reason)?,
            message: "Server closed the connection".to_string()
          })
        }
        p = PacketGameInfo => {
          nodes.set_selected_node(p.game.as_ref().and_then(|g| {
            let node = g.node.as_ref()?;
            node.id.clone()
          }))?;
          OutgoingMessage::CurrentGameInfo(p.game.extract()?)
        }
        p = PacketGamePlayerEnter => {
          OutgoingMessage::GamePlayerEnter(p)
        }
        p = PacketGamePlayerLeave => {
          OutgoingMessage::GamePlayerLeave(p)
        }
        p = PacketGameSlotUpdate => {
          OutgoingMessage::GameSlotUpdate(p)
        }
        p = PacketPlayerSessionUpdate => {
          if p.game_id.is_none() {
            nodes.set_selected_node(None)?;
          }
          *current_game_id.write() = p.game_id.clone();
          OutgoingMessage::PlayerSessionUpdate(S2ProtoUnpack::unpack(p)?)
        }
        p = PacketListNodes => {
          nodes.update_nodes(p.nodes.clone())?;
          let mut list = message::NodeList {
            nodes: Vec::with_capacity(p.nodes.len())
          };
          for node in p.nodes {
            list.nodes.push(message::Node {
              id: node.id,
              name: node.name,
              location: node.location,
              country_id: node.country_id,
              ping: nodes.get_current_ping(node.id),
            })
          }
          OutgoingMessage::ListNodes(list)
        }
        p = PacketGameSelectNode => {
          nodes.set_selected_node(p.node_id)?;
          OutgoingMessage::GameSelectNode(p)
        }
        p = PacketGamePlayerPingMapUpdate => {
          OutgoingMessage::GamePlayerPingMapUpdate(p)
        }
        p = PacketGamePlayerPingMapSnapshot => {
          OutgoingMessage::GamePlayerPingMapSnapshot(p)
        }
      }
    };

    Self::send_message(sender, msg).await
  }

  async fn send_message(sender: &WsSenderRef, msg: OutgoingMessage) -> Result<()> {
    if let Err(err) = sender.send(msg).await {
      tracing::error!("send event: {}", err);
      return Err(err);
    }
    Ok(())
  }
}

#[derive(Debug, S2ProtoEnum, PartialEq, Copy, Clone, Serialize)]
#[s2_grpc(proto_enum_type = "flo_net::proto::flo_connect::LobbyDisconnectReason")]
pub enum DisconnectReason {
  Unknown = 0,
  Multi = 1,
  Maintenance = 2,
}

#[derive(Debug, S2ProtoUnpack, Serialize)]
#[s2_grpc(message_type = "flo_net::proto::flo_connect::Session")]
pub struct PlayerSession {
  pub player: PlayerInfo,
  pub status: PlayerStatus,
  pub game_id: Option<i32>,
}

#[derive(Debug, S2ProtoUnpack, Serialize)]
#[s2_grpc(message_type = "flo_net::proto::flo_connect::PacketPlayerSessionUpdate")]
pub struct PlayerSessionUpdate {
  pub status: PlayerStatus,
  pub game_id: Option<i32>,
}

#[derive(Debug, S2ProtoEnum, PartialEq, Copy, Clone, Serialize)]
#[s2_grpc(proto_enum_type = "flo_net::proto::flo_connect::PlayerStatus")]
pub enum PlayerStatus {
  Idle = 0,
  InGame = 1,
}

#[derive(Debug, S2ProtoUnpack, Serialize)]
#[s2_grpc(message_type = "flo_net::proto::flo_connect::PlayerInfo")]
pub struct PlayerInfo {
  pub id: i32,
  pub name: String,
  pub source: PlayerSource,
}

#[derive(Debug, S2ProtoEnum, PartialEq, Copy, Clone, Serialize)]
#[s2_grpc(proto_enum_type = "flo_net::proto::flo_connect::PlayerSource")]
pub enum PlayerSource {
  Test = 0,
  BNet = 1,
}

#[derive(Debug, S2ProtoEnum, PartialEq, Copy, Clone, Serialize)]
#[s2_grpc(proto_enum_type = "flo_net::proto::flo_connect::ConnectLobbyRejectReason")]
pub enum RejectReason {
  Unknown = 0,
  ClientVersionTooOld = 1,
  InvalidToken = 2,
}

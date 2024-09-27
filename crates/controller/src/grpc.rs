use crate::config::{ApiRequestExt, GetInterceptor};
use crate::error::{Error, Result};
use crate::game::db::{CreateGameAsBotParams, CreateGameParams};
use crate::game::messages::{CreateGame, PlayerJoin, PlayerLeave};
use crate::game::state::cancel::CancelGame;
use crate::game::state::create::CreateGameAsBot;
use crate::game::state::node::SelectNode;
use crate::game::state::registry::{AddGamePlayer, Remove, RemoveGamePlayer, UpdateGameNodeCache};
use crate::game::state::start::{StartGameCheckAsBot, StartGameCheckAsBotResult};
use crate::node::messages::ListNode;
use crate::player::state::ping::GetPlayersPingSnapshot;
use crate::player::{PlayerBanType, PlayerSource, SourceState};
use crate::state::{ActorMapExt, ControllerStateRef};
use bs_diesel_utils::executor::ExecutorError;
use chrono::{DateTime, Utc};
use flo_grpc::controller::flo_controller_server::*;
use flo_grpc::controller::*;
use s2_grpc_utils::{S2ProtoEnum, S2ProtoPack, S2ProtoUnpack};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use tower_http::classify::GrpcFailureClass;
use tower_http::trace::TraceLayer;
use tracing::Span;

pub async fn serve(state: ControllerStateRef) -> Result<()> {
  let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, flo_constants::CONTROLLER_GRPC_PORT);
  let server_impl = FloControllerService::new(state.clone());

  let interceptor = state.config.send(GetInterceptor).await?;
  let server = FloControllerServer::with_interceptor(server_impl, interceptor);
  let layer = tower::ServiceBuilder::new()
    .layer(
      TraceLayer::new_for_grpc()
        .on_request(())
        .on_response(
          |response: &http::response::Response<_>, _latency: Duration, _span: &Span| {
            if response.headers().contains_key("grpc-status") {
              tracing::error!("controller-grpc: on_response: {:?}", response)
            }
          },
        )
        .on_body_chunk(())
        .on_eos(())
        .on_failure(()),
    )
    .into_inner();
  let server = Server::builder().layer(layer).add_service(server);
  server.serve(addr.into()).await?;
  Ok(())
}

pub struct FloControllerService {
  state: ControllerStateRef,
}

impl FloControllerService {
  pub fn new(state: ControllerStateRef) -> Self {
    FloControllerService { state }
  }
}

#[tonic::async_trait]
impl FloController for FloControllerService {
  async fn get_player(
    &self,
    request: Request<GetPlayerRequest>,
  ) -> Result<Response<GetPlayerReply>, Status> {
    let player_id = request.into_inner().player_id;
    let player = self
      .state
      .db
      .exec(move |conn| crate::player::db::get(conn, player_id))
      .await
      .map_err(Error::from)?;
    Ok(Response::new(GetPlayerReply {
      player: player.pack().map_err(Status::internal)?,
    }))
  }

  async fn get_player_by_token(
    &self,
    request: Request<GetPlayerByTokenRequest>,
  ) -> Result<Response<GetPlayerReply>, Status> {
    let token = request.into_inner().token;
    let player_id = crate::player::token::validate_player_token(&token)?.player_id;
    let player = self
      .state
      .db
      .exec(move |conn| crate::player::db::get(conn, player_id))
      .await
      .map_err(Error::from)?;
    Ok(Response::new(GetPlayerReply {
      player: player.pack().map_err(Status::internal)?,
    }))
  }

  async fn update_and_get_player(
    &self,
    request: Request<UpdateAndGetPlayerRequest>,
  ) -> Result<Response<UpdateAndGetPlayerReply>, Status> {
    use crate::player::db;
    let api_client_id = request.get_api_client_id();
    let mut req = request.into_inner();
    req.realm = Some(api_client_id.to_string());
    let upsert = db::UpsertPlayer {
      api_client_id,
      source: PlayerSource::unpack_enum(req.source()),
      name: req.name,
      source_id: req.source_id,
      source_state: {
        let state = req
          .source_state
          .map(|state| SourceState::unpack(state))
          .transpose()
          .map_err(Error::from)?;
        Some(serde_json::to_value(&state).map_err(Error::from)?)
      },
      realm: S2ProtoUnpack::unpack(req.realm).map_err(Error::from)?,
    };
    let player = self
      .state
      .db
      .exec(move |conn| db::upsert(conn, &upsert))
      .await
      .map_err(Error::from)?;
    let token = crate::player::token::create_player_token(player.id)?;
    Ok(Response::new(UpdateAndGetPlayerReply {
      player: player.pack().map_err(Status::internal)?,
      token,
    }))
  }

  async fn list_nodes(&self, _request: Request<()>) -> Result<Response<ListNodesReply>, Status> {
    let nodes = self.state.nodes.send(ListNode).await.map_err(Error::from)?;
    Ok(Response::new(ListNodesReply {
      nodes: nodes.pack().map_err(Error::from)?,
    }))
  }

  async fn list_games(
    &self,
    request: Request<ListGamesRequest>,
  ) -> Result<Response<ListGamesReply>, Status> {
    let params =
      crate::game::db::QueryGameParams::unpack(request.into_inner()).map_err(Status::internal)?;
    let r = self
      .state
      .db
      .exec(move |conn| crate::game::db::query(conn, &params))
      .await
      .map_err(|e| Status::internal(e.to_string()))?;

    Ok(Response::new(r.pack().map_err(Error::from)?))
  }

  async fn get_game(
    &self,
    request: Request<GetGameRequest>,
  ) -> Result<Response<GetGameReply>, Status> {
    let game_id = request.into_inner().game_id;
    let game = self
      .state
      .db
      .exec(move |conn| crate::game::db::get_full(conn, game_id))
      .await
      .map_err(|e| match e {
        ExecutorError::Task(Error::GameNotFound) => Status::invalid_argument(e.to_string()),
        other => Status::internal(other.to_string()),
      })?;
    Ok(Response::new(GetGameReply {
      game: game.pack().map_err(Error::from)?,
    }))
  }

  async fn create_game(
    &self,
    request: Request<CreateGameRequest>,
  ) -> Result<Response<CreateGameReply>, Status> {
    let game = self
      .state
      .games
      .send(CreateGame {
        params: CreateGameParams::unpack(request.into_inner()).map_err(Error::from)?,
      })
      .await
      .map_err(Error::from)??;

    Ok(Response::new(CreateGameReply {
      game: game.pack().map_err(Status::internal)?,
    }))
  }

  async fn join_game(
    &self,
    request: Request<JoinGameRequest>,
  ) -> Result<Response<JoinGameReply>, Status> {
    let params = request.into_inner();

    let game = self
      .state
      .games
      .send_to(
        params.game_id,
        PlayerJoin {
          player_id: params.player_id,
        },
      )
      .await?;

    self
      .state
      .games
      .send(AddGamePlayer {
        game_id: params.game_id,
        player_id: params.player_id,
      })
      .await
      .map_err(Error::from)?;

    Ok(Response::new(JoinGameReply {
      game: game.pack().map_err(Error::from)?,
    }))
  }

  async fn create_join_game_token(
    &self,
    request: Request<CreateJoinGameTokenRequest>,
  ) -> Result<Response<CreateJoinGameTokenReply>, Status> {
    let params = request.into_inner();
    let game_id = params.game_id;

    let game = self
      .state
      .db
      .exec(move |conn| crate::game::db::get(conn, game_id))
      .await
      .map_err(Error::from)?;

    if game.created_by.as_ref().map(|p| p.id) != Some(params.player_id) {
      return Err(Error::PlayerNotHost.into());
    }

    let token = crate::game::token::create_join_token(params.game_id)?;

    Ok(Response::new(CreateJoinGameTokenReply { token }))
  }

  async fn join_game_by_token(
    &self,
    request: Request<JoinGameByTokenRequest>,
  ) -> Result<Response<JoinGameReply>, Status> {
    let params = request.into_inner();
    let join_token = crate::game::token::validate_join_token(&params.token)?;

    let game = self
      .state
      .games
      .send_to(
        join_token.game_id,
        PlayerJoin {
          player_id: params.player_id,
        },
      )
      .await?;

    self
      .state
      .games
      .send(AddGamePlayer {
        game_id: join_token.game_id,
        player_id: params.player_id,
      })
      .await
      .map_err(Error::from)?;

    Ok(Response::new(JoinGameReply {
      game: game.pack().map_err(Error::from)?,
    }))
  }

  async fn leave_game(&self, request: Request<LeaveGameRequest>) -> Result<Response<()>, Status> {
    let params = request.into_inner();

    let res = self
      .state
      .games
      .send_to(
        params.game_id,
        PlayerLeave {
          player_id: params.player_id,
        },
      )
      .await
      .map_err(Error::from)?;

    if res.game_ended {
      tracing::debug!(
        game_id = params.game_id,
        "shutting down: reason: PlayerLeave"
      );
      self
        .state
        .games
        .send(Remove {
          game_id: params.game_id,
        })
        .await
        .map_err(Error::from)?;
    } else {
      self
        .state
        .games
        .send(RemoveGamePlayer {
          game_id: params.game_id,
          player_id: params.player_id,
        })
        .await
        .map_err(Error::from)?;
    }

    Ok(Response::new(()))
  }

  async fn select_game_node(
    &self,
    request: Request<SelectGameNodeRequest>,
  ) -> Result<Response<()>, Status> {
    let SelectGameNodeRequest {
      game_id,
      player_id,
      node_id,
    } = request.into_inner();

    self
      .state
      .games
      .send_to(
        game_id,
        SelectNode {
          player_id,
          node_id: node_id.clone(),
        },
      )
      .await?;

    self
      .state
      .games
      .notify(UpdateGameNodeCache { game_id, node_id })
      .await
      .map_err(Error::from)?;

    Ok(Response::new(()))
  }

  async fn cancel_game(&self, request: Request<CancelGameRequest>) -> Result<Response<()>, Status> {
    let req = request.into_inner();
    let game_id = req.game_id;
    let player_id = req.player_id;

    self
      .state
      .games
      .send_to(
        game_id,
        CancelGame {
          player_id: Some(player_id),
        },
      )
      .await?;

    tracing::debug!(game_id, "shutting down: reason: CancelGame");
    self
      .state
      .games
      .send(Remove { game_id })
      .await
      .map_err(Error::from)?;

    Ok(Response::new(()))
  }

  async fn import_map_checksums(
    &self,
    request: Request<ImportMapChecksumsRequest>,
  ) -> Result<Response<ImportMapChecksumsReply>, Status> {
    let items =
      Vec::<crate::map::db::ImportItem>::unpack(request.into_inner().items).map_err(Error::from)?;
    let updated = self
      .state
      .db
      .exec(move |conn| crate::map::db::import(conn, items))
      .await
      .map_err(Error::from)?;
    Ok(Response::new(ImportMapChecksumsReply {
      updated: updated as u32,
    }))
  }

  async fn search_map_checksum(
    &self,
    request: Request<SearchMapChecksumRequest>,
  ) -> Result<Response<SearchMapChecksumReply>, Status> {
    let sha1 = request.into_inner().sha1;
    let checksum = self
      .state
      .db
      .exec(move |conn| crate::map::db::search_checksum(conn, sha1))
      .await
      .map_err(Error::from)?;
    Ok(Response::new(SearchMapChecksumReply { checksum }))
  }

  async fn get_players_by_source_ids(
    &self,
    request: Request<GetPlayersBySourceIdsRequest>,
  ) -> Result<Response<GetPlayersBySourceIdsReply>, Status> {
    let api_client_id = request.get_api_client_id();
    let source_ids = request.into_inner().source_ids;
    let map = self
      .state
      .db
      .exec(move |conn| {
        crate::player::db::get_player_map_by_api_source_ids(conn, api_client_id, source_ids)
      })
      .await
      .map_err(Error::from)?;
    Ok(Response::new(GetPlayersBySourceIdsReply {
      player_map: map.pack().map_err(Error::from)?,
    }))
  }

  async fn get_player_ping_maps(
    &self,
    request: Request<GetPlayerPingMapsRequest>,
  ) -> Result<Response<GetPlayerPingMapsReply>, Status> {
    use flo_grpc::player::PlayerPingMap;
    use std::collections::HashMap;

    let ids = request.into_inner().ids;
    let snapshot = self
      .state
      .players
      .send(GetPlayersPingSnapshot { players: ids })
      .await
      .map_err(Error::from)?;

    Ok(Response::new(GetPlayerPingMapsReply {
      ping_maps: snapshot
        .map
        .into_iter()
        .map(|(player_id, map)| -> Result<_> {
          Ok(PlayerPingMap {
            player_id,
            ping_map: map
              .into_iter()
              .collect::<HashMap<_, _>>()
              .pack()
              .map_err(Error::from)?,
          })
        })
        .collect::<Result<Vec<_>>>()?,
    }))
  }

  async fn create_game_as_bot(
    &self,
    request: Request<CreateGameAsBotRequest>,
  ) -> Result<Response<CreateGameAsBotReply>, Status> {
    let game = self
      .state
      .games
      .send(CreateGameAsBot {
        api_client_id: request.get_api_client_id(),
        api_player_id: request.get_api_player_id(),
        params: CreateGameAsBotParams::unpack(request.into_inner()).map_err(Error::from)?,
      })
      .await
      .map_err(Error::from)??;

    Ok(Response::new(CreateGameAsBotReply {
      game: game.pack().map_err(Status::internal)?,
    }))
  }

  async fn start_game_as_bot(
    &self,
    request: Request<StartGameAsBotRequest>,
  ) -> Result<Response<StartGameAsBotReply>, Status> {
    use flo_net::proto::flo_connect::PacketGameStartPlayerClientInfoRequest;
    use std::collections::HashMap;
    use tokio::sync::oneshot;

    fn convert_map(
      map: HashMap<i32, PacketGameStartPlayerClientInfoRequest>,
    ) -> HashMap<i32, StartGamePlayerAck> {
      map
        .into_iter()
        .map(|(id, ack)| {
          (
            id,
            StartGamePlayerAck {
              war3_version: ack.war3_version,
              map_sha1: ack.map_sha1,
            },
          )
        })
        .collect()
    }

    let (tx, rx) = oneshot::channel();
    self
      .state
      .games
      .send_to(request.into_inner().game_id, StartGameCheckAsBot { tx })
      .await?;
    match rx.await {
      Ok(res) => match res {
        StartGameCheckAsBotResult::Started(map) => Ok(Response::new(StartGameAsBotReply {
          succeed: true,
          player_ack_map: map.map(convert_map).unwrap_or_default(),
          ..Default::default()
        })),
        StartGameCheckAsBotResult::Rejected(pkt) => Ok(Response::new(StartGameAsBotReply {
          succeed: false,
          error_message: pkt.message,
          player_ack_map: convert_map(pkt.player_client_info_map),
        })),
      },
      Err(_) => Err(Status::cancelled("System is shutting down")),
    }
  }

  async fn cancel_game_as_bot(
    &self,
    request: Request<CancelGameAsBotRequest>,
  ) -> Result<Response<()>, Status> {
    let player_id = request.get_api_player_id();
    self
      .cancel_game(Request::new(CancelGameRequest {
        game_id: request.into_inner().game_id,
        player_id,
      }))
      .await?;

    Ok(Response::new(()))
  }

  async fn reload(&self, _request: Request<()>) -> Result<Response<()>, Status> {
    self.state.reload().await?;
    Ok(Response::new(()))
  }

  async fn list_player_bans(
    &self,
    request: Request<ListPlayerBansRequest>,
  ) -> Result<Response<ListPlayerBansReply>, Status> {
    let api_client_id = request.get_api_client_id();
    let params = request.into_inner();
    let res = self
      .state
      .db
      .exec(move |conn| {
        crate::player::db::list_ban(conn, api_client_id, params.query.as_deref(), params.next_id)
      })
      .await
      .map_err(Error::from)?;
    Ok(Response::new(ListPlayerBansReply {
      player_bans: res.player_bans.pack().map_err(Status::internal)?,
      next_id: res.next_id,
    }))
  }

  async fn create_player_ban(
    &self,
    request: Request<CreatePlayerBanRequest>,
  ) -> Result<Response<()>, Status> {
    let api_client_id = request.get_api_client_id();
    let params = request.into_inner();
    let ban_expires_at = params
      .ban_expires_at
      .clone()
      .map(|t| DateTime::<Utc>::unpack(t))
      .transpose()
      .map_err(Status::internal)?;
    self
      .state
      .db
      .exec(move |conn| {
        crate::player::db::check_player_api_client_id(conn, api_client_id, params.player_id)?;
        crate::player::db::create_ban(
          conn,
          params.player_id,
          PlayerBanType::unpack_enum(params.ban_type()),
          ban_expires_at,
        )
      })
      .await
      .map_err(Error::from)?;
    Ok(Response::new(()))
  }

  async fn remove_player_ban(
    &self,
    request: Request<RemovePlayerBanRequest>,
  ) -> Result<Response<()>, Status> {
    let api_client_id = request.get_api_client_id();
    let params = request.into_inner();
    self
      .state
      .db
      .exec(move |conn| {
        crate::player::db::check_ban_api_client_id(conn, api_client_id, params.id)?;
        crate::player::db::remove_ban(conn, params.id)
      })
      .await
      .map_err(Error::from)?;
    Ok(Response::new(()))
  }
}

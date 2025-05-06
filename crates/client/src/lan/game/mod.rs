mod game;
mod lobby;
mod proxy;
pub mod slot;

pub use self::lobby::{LobbyAction, LobbyHandler};
pub use self::proxy::GameEndReason;
use crate::controller::ControllerClient;
use crate::error::*;
use crate::lan::game::proxy::PlayerEvent;
use crate::lan::game::slot::LanSlotInfo;
use crate::lan::get_lan_game_name;
use crate::node::stream::NodeConnectToken;
use crate::node::NodeInfo;
use flo_lan::{GameInfo, MdnsEvent, MdnsPublisher};
use flo_state::Addr;
use flo_task::SpawnScope;
use flo_types::game::LocalGameInfo;
use flo_types::node::{NodeGameStatus, SlotClientStatus};
use flo_w3gs::protocol::game::GameSettings;
use flo_w3map::MapChecksum;
use proxy::LanProxy;
use std::sync::Arc;
use tokio::sync::{mpsc, Notify};
use tracing_futures::Instrument;

pub struct LanGame {
  _scope: SpawnScope,
  state: Arc<State>,
  proxy: LanProxy,
  mdns_shutdown_notify: Arc<Notify>,
}

#[derive(Debug)]
pub struct LanGameInfo {
  pub(crate) game: Arc<LocalGameInfo>,
  pub(crate) slot_info: LanSlotInfo,
  pub(crate) map_checksum: MapChecksum,
  pub(crate) game_settings: GameSettings,
  pub(crate) lan_game_name_override: Option<String>,
}

impl LanGame {
  pub async fn create(
    game_version: String,
    my_player_id: i32,
    node: Arc<NodeInfo>,
    player_token: Vec<u8>,
    game: Arc<LocalGameInfo>,
    map_checksum: MapChecksum,
    client: Addr<ControllerClient>,
    save_replay: bool,
    user_replay_path: String,
    lobby_countdown_notify: Option<Arc<Notify>>,
  ) -> Result<Self> {
    let mdns_shutdown_notify = Arc::new(Notify::new());

    let game_id = game.game_id;
    let game_name = get_lan_game_name(&game.name, my_player_id);
    let mut game_info = GameInfo::new(
      game.game_id,
      &game_name,
      &game.map_path.replace("\\", "/"),
      game.map_sha1,
      map_checksum.xoro,
    )?;
    let token = NodeConnectToken::from_vec(player_token).ok_or_else(|| Error::InvalidNodeToken)?;

    let proxy = LanProxy::start(
      LanGameInfo {
        slot_info: crate::lan::game::slot::build_player_slot_info(
          my_player_id,
          game.random_seed,
          &game.slots,
          game.map_twelve_p,
        )?,
        game,
        map_checksum,
        game_settings: game_info.data.settings.clone(),
        lan_game_name_override: None,
      },
      node,
      token,
      client.clone(),
      game_version.clone(),
      save_replay,
      user_replay_path,
      lobby_countdown_notify,
    )
    .await?;
    game_info.set_port(proxy.port());
    let scope = SpawnScope::new();
    let state = Arc::new(State {
      game_id,
      my_player_id,
    });
    tokio::spawn(
      {
        let mut scope = scope.handle();
        let mdns_shutdown_notify = mdns_shutdown_notify.clone();
        let client_clone = client.clone();

        // Create a channel for MDNS events
        let (event_tx, mut event_rx) = mpsc::channel::<MdnsEvent>(10);

        // Forward MDNS events to the client
        tokio::spawn(async move {
          while let Some(event) = event_rx.recv().await {
            match event {
              MdnsEvent::Error(error_msg) => {
                tracing::error!("MDNS error in LanGame create: {}", error_msg);
                if let Err(err) = client_clone
                  .send(crate::lan::LanEvent::MdnsError {
                    game_id,
                    error: error_msg,
                  })
                  .await
                {
                  tracing::error!("Failed to send MdnsError in LanGame create: {}", err);
                }
              }
            }
          }
        });

        let publisher = MdnsPublisher::start(game_version, game_info, event_tx).await?;
        async move {
          let _publisher = publisher;
          tokio::select! {
            _ = scope.left() => {}
            _ = mdns_shutdown_notify.notified() => {}
          }

          // sleep(Duration::from_secs(1)).await;

          tracing::debug!("exiting")
        }
      }
      .instrument(tracing::debug_span!("publisher_worker")),
    );

    Ok(Self {
      _scope: scope,
      proxy,
      state,
      mdns_shutdown_notify,
    })
  }

  pub fn game_id(&self) -> i32 {
    self.state.game_id
  }

  pub async fn update_game_status(&self, status: NodeGameStatus) {
    self.proxy.dispatch_game_status_change(status).await;
  }

  pub async fn update_player_status(&mut self, player_id: i32, status: SlotClientStatus) {
    self
      .proxy
      .dispatch_player_event(PlayerEvent::PlayerStatusChange { player_id, status })
      .await;
  }

  pub fn is_same_game(&self, game_id: i32, my_player_id: i32) -> bool {
    self.state.game_id == game_id && self.state.my_player_id == my_player_id
  }

  pub fn shutdown(self) {
    self.mdns_shutdown_notify.notify_one();
    tokio::spawn(async move {
      if let Err(_) =
        tokio::time::timeout(std::time::Duration::from_secs(10), self.proxy.shutdown()).await
      {
        tracing::error!("shutdown last lan game timeout.");
      }
    });
  }
}

struct State {
  game_id: i32,
  my_player_id: i32,
}

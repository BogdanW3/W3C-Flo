use crate::error::*;
use crate::game::state::GameActor;
use crate::node::messages as node_messages;
use crate::state::ActorMapExt;
use diesel::prelude::*;

use crate::player::state::sender::PlayerFrames;

use flo_net::packet::FloPacket;

use flo_state::{async_trait, Context, Handler, Message};

pub struct CancelGame {
  pub player_id: Option<i32>,
}

impl Message for CancelGame {
  type Result = Result<()>;
}

#[async_trait]
impl Handler<CancelGame> for GameActor {
  async fn handle(
    &mut self,
    _: &mut Context<Self>,
    CancelGame { player_id }: CancelGame,
  ) -> Result<()> {
    let game_id = self.game_id;

    let active_players = self
      .db
      .exec(move |conn| {
        conn.transaction(|| -> Result<Vec<i32>> {
          // Cancel the game first
          crate::game::db::cancel(conn, game_id, player_id)?;

          // Get active players who haven't left yet
          let active_players = crate::game::db::get_node_active_player_ids(conn, game_id)?;

          // Then handle player leave for each active player so that they dont remain in the slot table
          for active_player_id in &active_players {
            crate::game::db::leave_node(conn, game_id, *active_player_id)?;
          }

          // Return players who were still active when the game was cancelled
          Ok(active_players)
        })
      })
      .await
      .map_err(Error::from)?;

    // After the game has been cancelled, notify nodes about players who left
    for player_id in &active_players {
      let node_id = self.selected_node_id.clone().unwrap();

      // Notify node about player leave
      if let Err(err) = self
        .nodes
        .send_to(
          node_id,
          node_messages::NodePlayerLeave {
            game_id,
            player_id: *player_id,
          },
        )
        .await
      {
        tracing::error!(
          game_id,
          node_id,
          player_id,
          "force leave node error during game cancellation: {:?}",
          err
        );
      }
    }

    // Notify player registry about players having left
    self
      .player_reg
      .players_leave_game(self.players.clone(), game_id)
      .await?;

    // Notify clients
    let packet_iter: std::vec::IntoIter<(i32, PlayerFrames)> = self
      .players
      .iter()
      .cloned()
      .map(|player_id| {
        use flo_net::proto::flo_connect::*;
        let frame_left = PacketGamePlayerLeave {
          game_id,
          player_id,
          reason: PlayerLeaveReason::GameCancelled.into(),
        }
        .encode_as_frame()?;

        Ok((player_id, PlayerFrames::from(frame_left)))
      })
      .collect::<Result<Vec<_>>>()?
      .into_iter();

    self.player_reg.broadcast_map(packet_iter).await?;

    Ok(())
  }
}

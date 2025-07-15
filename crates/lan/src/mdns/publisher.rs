use crate::error::*;
use crate::game_info::GameInfo;
use futures::future::TryFutureExt;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing_futures::Instrument;

type GameInfoRef = Arc<RwLock<GameInfo>>;
type UpdateTx = mpsc::Sender<oneshot::Sender<()>>;

/// Events that can be emitted by the MDNS publisher
#[derive(Debug, Clone)]
pub enum MdnsEvent {
  /// An error occurred in the MDNS publisher worker
  Error(String),
}

/// Type alias for the MDNS event sender
pub type MdnsEventSender = mpsc::Sender<MdnsEvent>;

#[derive(Debug)]
pub struct MdnsPublisher {
  update_tx: UpdateTx,
  game_info: GameInfoRef,
}

impl MdnsPublisher {
  pub async fn start(
    game_version: String,
    game_info: GameInfo,
    event_tx: MdnsEventSender,
  ) -> Result<Self> {
    let game_name = game_info.name.to_string_lossy().to_string();
    let game_info = Arc::new(RwLock::new(game_info));
    let (update_tx, update_rx) = mpsc::channel::<oneshot::Sender<()>>(1);

    tokio::spawn(
      Self::worker(game_version, game_info.clone(), game_name, update_rx)
        .map_err(move |err| {
          tracing::error!("Mdns worker exited with error: {}", err);
          let _ = event_tx.try_send(MdnsEvent::Error(err.to_string()));
        })
        .instrument(tracing::debug_span!("worker")),
    );

    Ok(Self {
      update_tx,
      game_info,
    })
  }

  async fn worker(
    game_version: String,
    game_info: GameInfoRef,
    game_name: String,
    mut update_rx: mpsc::Receiver<oneshot::Sender<()>>,
  ) -> Result<()> {
    let name = if game_name.bytes().len() > 31 {
      let name = game_name
        .char_indices()
        .filter_map(|(i, c)| {
          if i < 31 || (i == 31 && c.len_utf8() == 1) {
            Some(c)
          } else {
            None
          }
        })
        .collect::<String>();
      name
    } else {
      game_name
    };

    use async_dnssd::{register_extended, RegisterData, Type};

    let (port, data) = {
      let mut game_info = game_info.write();
      game_info.message_id = game_info.message_id + 1;
      (game_info.data.port, game_info.encode_to_bytes()?)
    };
    let reg_type = super::get_reg_type(&game_version)?;
    let reg = register_extended(
      &reg_type,
      port,
      RegisterData {
        flags: async_dnssd::RegisterFlags::NO_AUTO_RENAME | async_dnssd::RegisterFlags::UNIQUE,
        name: Some(&name),
        ..Default::default()
      },
    )
    .map_err(Error::BonjourRegister)?;

    let record = reg
      .add_record(Type(66), &data, 4500)
      .map_err(Error::BonjourRegister)?;

    let (_reg, res) = reg.await.map_err(Error::BonjourRegister)?;

    tracing::debug!("register result: {:?}", res);

    loop {
      tokio::select! {
        update = update_rx.recv() => {
          tracing::debug!("update");
          if let Some(ack) = update {
            let data = {
              let mut game_info = game_info.write();
              game_info.message_id = game_info.message_id + 1;
              game_info.encode_to_bytes()?
            };
            record.update_record(&data, 4500).map_err(|err| Error::BonjourUpdate(err.to_string()))?;
            ack.send(()).ok();
          } else {
            tracing::debug!("update handle dropped");
            break;
          }
        },
      }
    }

    tracing::info!("MdnsPublisher worker: exiting");
    Ok(())
  }

  pub async fn update<F>(&mut self, f: F) -> Result<()>
  where
    F: FnOnce(&mut GameInfo),
  {
    {
      let mut lock = self.game_info.write();
      f(&mut lock)
    }
    self.refresh().await?;
    Ok(())
  }

  pub async fn refresh(&mut self) -> Result<()> {
    let (ack_tx, ack_rx) = oneshot::channel();
    self
      .update_tx
      .send(ack_tx)
      .await
      .map_err(|_| Error::BonjourUpdate("worker dead: send".to_string()))?;

    tokio::time::timeout(Duration::from_secs(1), ack_rx)
      .await
      .map_err(|_| Error::BonjourUpdate("timeout".to_string()))?
      .map_err(|_| Error::BonjourUpdate("worker dead: recv".to_string()))
  }
}

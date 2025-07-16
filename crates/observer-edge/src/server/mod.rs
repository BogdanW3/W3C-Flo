pub mod peer;
mod send_queue;

use crate::dispatcher::{CreateGameStreamServer, GetGameInfo};
use crate::error::Error;
use crate::error::Result;
use crate::Dispatcher;
use flo_net::{listener::FloListener, observer::ObserverConnectRejectReason, stream::FloStream};
use flo_state::Addr;
use std::time::SystemTime;
use tokio_stream::StreamExt;

pub struct StreamServer {
  listener: FloListener,
  dispatcher: Addr<Dispatcher>,
}

impl StreamServer {
  pub async fn new(dispatcher: Addr<Dispatcher>) -> Result<Self> {
    let listener = FloListener::bind_v4(flo_constants::OBSERVER_SOCKET_PORT).await?;
    Ok(Self {
      listener,
      dispatcher,
    })
  }

  pub async fn serve(mut self) -> Result<()> {
    while let Some(transport) = self.listener.incoming().try_next().await? {
      let dispatcher = self.dispatcher.clone();

      // Spawn the handler in a separate async block to simplify trait resolution
      tokio::spawn(Self::handle_connection(dispatcher, transport));
    }
    Ok(())
  }

  // Extract the handler logic into a separate async function to avoid complex trait resolution
  async fn handle_connection(dispatcher: Addr<Dispatcher>, transport: FloStream) {
    let handler = Handler {
      dispatcher,
      transport,
    };

    if let Err(err) = handler.run().await {
      tracing::error!("stream handler: {}", err);
    }
  }
}

// Explicitly implement Send for Handler to help the compiler
struct Handler {
  dispatcher: Addr<Dispatcher>,
  transport: FloStream,
}

impl Handler {
  async fn run(mut self) -> Result<()> {
    let accepted = match self.accept().await? {
      Some(v) => v,
      None => {
        return Ok(());
      }
    };

    let server = self
      .dispatcher
      .send(CreateGameStreamServer {
        game_id: accepted.game_id,
        delay_secs: accepted.delay_secs,
      })
      .await??;

    server.run(self.transport).await?;

    Ok(())
  }

  async fn accept(&mut self) -> Result<Option<Accepted>> {
    use flo_net::observer::{PacketObserverConnect, PacketObserverConnectAccept, Version};
    let connect: PacketObserverConnect = self.transport.recv().await?;
    let token = match flo_observer::token::validate_observer_token(&connect.token) {
      Ok(v) => v,
      Err(_) => {
        self
          .reject(ObserverConnectRejectReason::InvalidToken, None)
          .await?;
        return Ok(None);
      }
    };
    let dispatcher_send_result = self
      .dispatcher
      .send(GetGameInfo {
        game_id: token.game_id,
      })
      .await;

    let (meta, game) = match dispatcher_send_result {
      Ok(result) => match result {
        Ok(game) => game,
        Err(err) => {
          match err {
            Error::GameNotFound(_) => {
              self
                .reject(ObserverConnectRejectReason::GameNotFound, None)
                .await?;
            }
            err => {
              tracing::error!(game_id = token.game_id, "get game: {}", err);
              self
                .reject(ObserverConnectRejectReason::GameNotReady, None)
                .await?;
            }
          }
          return Ok(None);
        }
      },
      Err(err) => {
        tracing::error!(game_id = token.game_id, "dispatcher send error: {}", err);
        self
          .reject(ObserverConnectRejectReason::GameNotReady, None)
          .await?;
        return Ok(None);
      }
    };

    // If a game is password protected, request the password from the client
    if let Some(expected_password_hash) = &game.flo_tv_password_sha256 {
      if expected_password_hash.is_empty() {
        // Skip password protection for empty password hash
      } else {
        use flo_net::observer::{PacketObserverPasswordRequest, PacketObserverPasswordResponse};
        use tokio::time::{timeout, Duration};

        // Send "password requested" packet to the client
        self
          .transport
          .send(PacketObserverPasswordRequest {})
          .await?;

        // Await password response with a 120 seconds timeout
        let password_response = match timeout(
          Duration::from_secs(120),
          self.transport.recv::<PacketObserverPasswordResponse>(),
        )
        .await
        {
          Ok(Ok(response)) => response,
          Ok(Err(err)) => {
            tracing::error!(game_id = token.game_id, "password response error: {}", err);
            self
              .reject(ObserverConnectRejectReason::PasswordResponseError, None)
              .await?;
            return Ok(None);
          }
          Err(_) => {
            tracing::info!(game_id = token.game_id, "password response timeout");
            self
              .reject(ObserverConnectRejectReason::PasswordTimeout, None)
              .await?;
            return Ok(None);
          }
        };

        // Verify password by comparing SHA256 hashes
        if &password_response.password_sha256 != expected_password_hash {
          tracing::info!(game_id = token.game_id, "incorrect password provided");
          self
            .reject(ObserverConnectRejectReason::IncorrectPassword, None)
            .await?;
          return Ok(None);
        }

        tracing::info!(game_id = token.game_id, "password verification successful");
      }
    }

    let start_time = meta.started_at.timestamp();
    let now = (SystemTime::now().duration_since(SystemTime::UNIX_EPOCH))
      .unwrap()
      .as_secs() as i64;
    let expected = start_time + token.delay_secs.unwrap_or_default();

    if expected > now {
      self
        .reject(ObserverConnectRejectReason::DelayNotOver, expected.into())
        .await?;
      return Ok(None);
    }

    self
      .transport
      .send(PacketObserverConnectAccept {
        version: Some(Version {
          major: crate::version::FLO_OBSERVER_VERSION.major,
          minor: crate::version::FLO_OBSERVER_VERSION.minor,
          patch: crate::version::FLO_OBSERVER_VERSION.patch,
        }),
        game: Some(game),
        delay_secs: token.delay_secs.clone(),
      })
      .await?;

    Ok(Some(Accepted {
      game_id: token.game_id,
      delay_secs: token.delay_secs,
    }))
  }

  async fn reject(
    &mut self,
    reason: ObserverConnectRejectReason,
    delay_ends_at: Option<i64>,
  ) -> Result<()> {
    use flo_net::observer::PacketObserverConnectReject;
    self
      .transport
      .send({
        let mut pkt = PacketObserverConnectReject {
          delay_ends_at,
          ..Default::default()
        };
        pkt.set_reason(reason);
        pkt
      })
      .await?;
    Ok(())
  }
}

struct Accepted {
  game_id: i32,
  delay_secs: Option<i64>,
}

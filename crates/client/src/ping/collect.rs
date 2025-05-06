use super::{PingError, SendPing};
use flo_state::{async_trait, Actor, Addr, Context, Handler, Message};
use flo_types::ping::PingStats;
use futures::future::AbortHandle;
use rand::Rng;
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::error::SendTimeoutError;
use tokio::sync::mpsc::Sender;
use tokio::sync::watch;
use tokio::time::sleep;

const RESULT_BUFFER_SIZE: usize = 256;
const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(15);
const ACTIVE_PING_INTERVAL: Duration = Duration::from_secs(5);
const WARMUP_PING_INTERVAL: Duration = Duration::from_secs(1);
const WARMUP_DURATION: Duration = Duration::from_secs(60);
const PING_TIMEOUT: Duration = Duration::from_secs(3);
const OFFLINE_THRESHOLD: Duration = Duration::from_secs(2 * 60); // 2 minutes

#[derive(Debug, Clone)]
struct PingResult {
  rtt: Option<u32>, // Round-trip time in ms, None if timeout/error
}

pub struct PingCollectActor {
  sock_addr_string: String,
  sock_addr: SocketAddr,
  results: VecDeque<PingResult>,
  in_flight_pings: BTreeMap<u32, (AbortHandle, u8)>, // Map of timestamp -> (timeout handle, send_counter)
  sender: Sender<SendPing>,
  ping_now_tx: Option<watch::Sender<bool>>, // Signal to wake up the main loop
  cached_stats: Option<PingStats>,          // Last computed stats
  is_warming_up: bool,
  is_active: bool,
  last_successful_ping: Option<Instant>, // When we last got a successful ping
  random_id: [u8; 3],                    // Random 3-byte identifier for this actor
  send_counter: AtomicU8,                // Rolling counter for sent pings
}

impl PingCollectActor {
  pub fn new(sender: Sender<SendPing>, sock_addr: SocketAddr) -> Self {
    // Generate a random 3-byte identifier
    let mut random_id = [0u8; 3];
    rand::thread_rng().fill(&mut random_id[..]);
    // Initialize send_counter with a random value
    let initial_counter = rand::thread_rng().gen::<u8>();

    Self {
      sender,
      sock_addr_string: format!("{}", sock_addr),
      sock_addr,
      results: VecDeque::with_capacity(RESULT_BUFFER_SIZE),
      in_flight_pings: BTreeMap::new(),
      ping_now_tx: None,
      cached_stats: None,
      is_warming_up: false,
      is_active: false,
      last_successful_ping: None,
      random_id,
      send_counter: AtomicU8::new(initial_counter),
    }
  }

  fn address_str(&self) -> &str {
    self.sock_addr_string.as_str()
  }

  fn get_interval(&self) -> Duration {
    if self.is_warming_up {
      WARMUP_PING_INTERVAL
    } else if self.is_active {
      ACTIVE_PING_INTERVAL
    } else {
      DEFAULT_PING_INTERVAL
    }
  }

  // Add a ping result to the buffer, maintaining fixed size
  fn add_result(&mut self, rtt: Option<u32>, ctx: &mut Context<Self>) {
    let result = PingResult { rtt };

    // Update last successful ping time for offline detection
    if let Some(_rtt_value) = rtt {
      self.check_offline_status(ctx);
      self.last_successful_ping = Some(Instant::now());
    }

    if self.results.len() >= RESULT_BUFFER_SIZE {
      self.results.pop_front();
    }
    self.results.push_back(result);

    // Invalidate cache when results change
    self.cached_stats = None;
  }

  // Check if we've been offline for too long and need to restart warmup
  fn check_offline_status(&mut self, ctx: &mut Context<Self>) {
    if !self.is_warming_up {
      if let Some(last_success) = self.last_successful_ping {
        if Instant::now().duration_since(last_success) > OFFLINE_THRESHOLD {
          tracing::info!(
            address = self.address_str(),
            "Node is back online after being offline for {:?}s, restarting warmup",
            Instant::now().duration_since(last_success).as_secs()
          );
          self.restart_warmup(ctx);
        }
      }
    }
  }

  // Restart the warmup procedure
  fn restart_warmup(&mut self, ctx: &mut Context<Self>) {
    // Clear all measurements
    self.results.clear();
    self.cached_stats = None;

    // start warmup
    self.start_warmup_task(ctx);

    // Signal for an immediate ping
    self.ping_now();
  }

  // Schedule warmup timeout task
  fn start_warmup_task(&mut self, ctx: &mut Context<Self>) {
    self.is_warming_up = true;
    tracing::info!(address = self.address_str(), "Starting warmup");
    let addr = ctx.addr();
    ctx.spawn(async move {
      sleep(WARMUP_DURATION).await;
      addr.send(WarmupCompleted).await.ok();
    });
  }

  // Calculate stats from all results
  fn collect_stats(&mut self, get_raw_stats: bool) -> PingStats {
    if self.results.is_empty() {
      return PingStats {
        min: None,
        max: None,
        avg: None,
        current: None,
        loss_rate: 1.0,
      };
    }

    if !get_raw_stats {
      // Use cached stats if available
      if let Some(stats) = &self.cached_stats {
        return stats.clone();
      }
    }

    // Get all valid RTT measurements
    let valid_rtts: Vec<u32> = self
      .results
      .iter()
      .filter_map(|result| result.rtt)
      .collect();

    // Calculate stats
    let total_count = self.results.len() as f32;
    let valid_count = valid_rtts.len() as f32;

    let min = valid_rtts.iter().min().cloned();

    // Calculate 95th percentile instead of absolute maximum, just to smooth out the absolute extremes
    let max = if !valid_rtts.is_empty() {
      let p95_idx = ((valid_rtts.len() - 1) as f32 * 0.95) as usize;
      let mut indices: Vec<_> = (0..valid_rtts.len()).collect();

      // Use partial sort to find p95 value without cloning the values
      indices.select_nth_unstable_by_key(p95_idx, |&i| valid_rtts[i]);
      Some(valid_rtts[indices[p95_idx]])
    } else {
      None
    };

    let avg = if !valid_rtts.is_empty() {
      Some(valid_rtts.iter().sum::<u32>() / valid_rtts.len() as u32)
    } else {
      None
    };

    let mut loss_rate = (total_count - valid_count) / total_count;

    // Current is the most recent successful ping
    let current = self.results.iter().rev().find_map(|result| result.rtt);

    // If we don't want raw stats but the last ping was a failure, set loss to 1.0
    // to indicate that the node is offline
    if !get_raw_stats && current.is_none() {
      loss_rate = 1.0;
    }

    let stats = PingStats {
      min,
      max,
      avg,
      current,
      loss_rate,
    };

    // Cache the computed stats unless these were requested as raw
    if !get_raw_stats {
      self.cached_stats = Some(stats.clone());
    }
    stats
  }

  // Send a ping packet
  async fn send_ping(&mut self, ctx: &mut Context<Self>) {
    // Current timestamp as milliseconds since UNIX epoch
    let now = SystemTime::now();
    let now_ms = now
      .duration_since(UNIX_EPOCH)
      .unwrap_or_else(|_| Duration::from_secs(0))
      .as_millis() as u32;

    // Atomically load and increment the send counter (wraps around on overflow)
    let current_send_counter = self.send_counter.fetch_add(1, Ordering::Relaxed);

    // Create the ping packet (8 bytes total):
    // [0..4]: timestamp (u32)
    // [4]: send_counter (u8)
    // [5..8]: random_id ([u8; 3])
    let mut buf = [0_u8; 8];
    buf[0..4].copy_from_slice(&now_ms.to_le_bytes());
    buf[4] = current_send_counter;
    buf[5..8].copy_from_slice(&self.random_id);

    // Send the ping
    match self
      .sender
      .send_timeout(
        SendPing {
          to: self.sock_addr,
          data: buf,
        },
        Duration::from_millis(50),
      )
      .await
    {
      Ok(_) => {
        tracing::trace!(
          address = self.address_str(),
          "Ping sent with timestamp {}",
          now_ms
        );

        // Set up timeout
        let addr = ctx.addr();
        let (future, abort_handle) = futures::future::abortable(async move {
          sleep(PING_TIMEOUT).await;
          addr.send(PingTimeout { timestamp: now_ms }).await.ok();
        });

        // Store the abort handle and the send_counter for this specific ping
        self
          .in_flight_pings
          .insert(now_ms, (abort_handle, current_send_counter));

        // Spawn the timeout task
        ctx.spawn(async {
          future.await.ok();
        });
      }
      Err(err) => {
        let error = match err {
          SendTimeoutError::Timeout(_) => PingError::SenderTimeout,
          SendTimeoutError::Closed(_) => PingError::SenderGone,
        };
        tracing::error!(address = self.address_str(), error_type = ?error, "Ping error: {}", error);
        self.add_result(None, ctx);
      }
    }
  }

  // Signal the main loop to ping immediately
  fn ping_now(&self) {
    if let Some(tx) = &self.ping_now_tx {
      let _ = tx.send(true); // Signal to wake up and ping now
    } else {
      tracing::error!(
        address = self.address_str(),
        "Unable to emit ping now signal: ping_now_tx is None"
      );
    }
  }

  // Main ping loop
  async fn run_ping_loop(
    addr: Addr<Self>,
    sock_addr_string: String,
    mut rx: watch::Receiver<bool>,
  ) {
    // Send the start warmup message
    addr.send(StartWarmup).await.ok();

    loop {
      // Get the current interval from the actor
      let interval = match addr.send(GetInterval).await {
        Ok(interval) => interval,
        Err(err) => {
          tracing::error!(address = sock_addr_string, error = ?err, "Actor is gone during GetInterval, stopping ping loop");
          break;
        }
      };

      // Wait for either the interval to pass or a signal to ping now
      match tokio::time::timeout(interval, rx.changed()).await {
        Ok(Ok(_)) => {
          // Signal received, check if we need to ping (we don't care about the value)
          let _ = rx.borrow_and_update();
        }
        Ok(Err(err)) => {
          tracing::debug!(address = sock_addr_string, error = ?err, "ping now channel dropped");
          break;
        }
        Err(_) => {
          // Timeout occurred, time for regular ping
        }
      }

      // Send ping message to the actor
      if addr.send(PerformPing).await.is_err() {
        break; // Actor is gone
      }
    }
  }
}

#[async_trait]
impl Actor for PingCollectActor {
  async fn started(&mut self, ctx: &mut Context<Self>) {
    // Create the channel for signaling immediate pings
    let (tx, rx) = watch::channel(false);
    self.ping_now_tx = Some(tx);

    // Start the main ping loop with just the actor address
    let addr = ctx.addr().clone();
    let sock_addr_str = self.sock_addr_string.clone();

    // Generate the random delay value outside the async block
    let random_delay = rand::thread_rng().gen_range(1..1000);

    ctx.spawn(async move {
      // Introduce a random delay since we're publishing via a single socket
      tokio::time::sleep(Duration::from_millis(random_delay)).await;
      return Self::run_ping_loop(addr, sock_addr_str, rx).await;
    });
  }
}

// Message to get current interval
struct GetInterval;
impl Message for GetInterval {
  type Result = Duration;
}

#[async_trait]
impl Handler<GetInterval> for PingCollectActor {
  async fn handle(
    &mut self,
    _: &mut Context<Self>,
    _: GetInterval,
  ) -> <GetInterval as Message>::Result {
    self.get_interval()
  }
}

// Message to notify when warmup is completed
struct WarmupCompleted;
impl Message for WarmupCompleted {
  type Result = ();
}

#[async_trait]
impl Handler<WarmupCompleted> for PingCollectActor {
  async fn handle(
    &mut self,
    _: &mut Context<Self>,
    _: WarmupCompleted,
  ) -> <WarmupCompleted as Message>::Result {
    if self.is_warming_up {
      let stats = self.collect_stats(true);
      // This should be part of PingStats, but I don't feel like making a PR to flo-grpc right now
      let valid_count = self.results.iter().filter_map(|result| result.rtt).count();
      let total_count = self.results.len();
      tracing::info!(
        address = self.address_str(),
        "Warmup period completed: {:?}/{:?}: Stats: {:?}",
        valid_count,
        total_count,
        stats
      );
      self.is_warming_up = false;
    }
  }
}

// Message to perform a ping (from the loop)
struct PerformPing;

impl Message for PerformPing {
  type Result = ();
}

#[async_trait]
impl Handler<PerformPing> for PingCollectActor {
  async fn handle(
    &mut self,
    ctx: &mut Context<Self>,
    _: PerformPing,
  ) -> <PerformPing as Message>::Result {
    self.send_ping(ctx).await;
  }
}

// Message to request an immediate ping
pub struct PingNow;
impl Message for PingNow {
  type Result = ();
}

#[async_trait]
impl Handler<PingNow> for PingCollectActor {
  async fn handle(&mut self, _: &mut Context<Self>, _: PingNow) -> <PingNow as Message>::Result {
    self.ping_now();
  }
}

// Timeout message
struct PingTimeout {
  timestamp: u32,
}

impl Message for PingTimeout {
  type Result = ();
}

#[async_trait]
impl Handler<PingTimeout> for PingCollectActor {
  async fn handle(
    &mut self,
    ctx: &mut Context<Self>,
    PingTimeout { timestamp }: PingTimeout,
  ) -> <PingTimeout as Message>::Result {
    // Only process if this ping is still in flight
    if let Some((abort_handle, _)) = self.in_flight_pings.remove(&timestamp) {
      // Although the timeout future already completed, explicitly aborting is good practice
      abort_handle.abort();
      self.add_result(None, ctx);
    } else {
      tracing::debug!(
        address = self.address_str(),
        timestamp = timestamp,
        "Received timeout for unknown ping timestamp"
      );
    }
  }
}

// Handle ping replies
pub struct PingReply(pub [u8; 8], pub u32);
impl Message for PingReply {
  type Result = ();
}

#[async_trait]
impl Handler<PingReply> for PingCollectActor {
  async fn handle(
    &mut self,
    ctx: &mut Context<Self>,
    PingReply(bytes, receive_timestamp): PingReply,
  ) -> <PingReply as Message>::Result {
    // Extract data from the 8-byte reply:
    // [0..4]: timestamp (u32)
    // [4]: send_counter (u8)
    // [5..8]: random_id ([u8; 3])
    let mut timestamp_bytes = [0_u8; 4];
    timestamp_bytes.copy_from_slice(&bytes[0..4]);
    let sent_time = u32::from_le_bytes(timestamp_bytes);

    let received_send_counter = bytes[4];

    let mut id_bytes = [0u8; 3];
    id_bytes.copy_from_slice(&bytes[5..8]);

    // Verify that the random ID matches
    if id_bytes != self.random_id {
      tracing::error!(
        address = self.address_str(),
        timestamp = sent_time,
        expected_id = ?self.random_id,
        received_id = ?id_bytes,
        "Received ping reply with incorrect random ID"
      );
      return;
    }

    let rtt = receive_timestamp.saturating_sub(sent_time); // Use saturating_sub for safety

    // Check if the ping is still in flight using the timestamp, but don't remove it yet.
    if let Some((_, expected_send_counter)) = self.in_flight_pings.get(&sent_time) {
      // Verify the send_counter first.
      if received_send_counter != *expected_send_counter {
        tracing::error!(
          address = self.address_str(),
          timestamp = sent_time,
          expected_counter = *expected_send_counter,
          received_counter = received_send_counter,
          "Received ping reply with incorrect send counter. Ignoring."
        );
        // Don't remove the entry, don't add result.
        return;
      }

      // Counter matches, now remove the entry.
      // We need to re-fetch it because the map might have changed between the get and remove.
      // This should always succeed since we just checked it exists and didn't yield control.
      if let Some((abort_handle, _)) = self.in_flight_pings.remove(&sent_time) {
        // Abort the corresponding timeout task.
        abort_handle.abort();
        // Proceed with adding the result if the timestamp is valid.
        if sent_time <= receive_timestamp {
          self.add_result(Some(rtt), ctx);
        } else {
          tracing::error!(
            address = self.address_str(),
            timestamp = sent_time,
            current_time = receive_timestamp,
            rtt = rtt,
            "Invalid timestamp in reply (timestamp > current time)"
          );
        }
      } else {
        // This case should theoretically not happen if the code runs sequentially
        tracing::warn!(
          address = self.address_str(),
          timestamp = sent_time,
          "In-flight ping disappeared between check and remove. Race condition?"
        );
      }
    } else {
      tracing::debug!(
        address = self.address_str(),
        timestamp = sent_time,
        rtt = rtt,
        received_send_counter = received_send_counter,
        current_send_counter = self.send_counter.load(Ordering::Relaxed),
        "Received reply for already processed ping timestamp - UDP duplicate?"
      );
    }
  }
}

// Get ping stats
pub struct GetPingStats;

impl Message for GetPingStats {
  type Result = (SocketAddr, PingStats);
}

#[async_trait]
impl Handler<GetPingStats> for PingCollectActor {
  async fn handle(
    &mut self,
    _: &mut Context<Self>,
    _: GetPingStats,
  ) -> <GetPingStats as Message>::Result {
    (self.sock_addr, self.collect_stats(false))
  }
}

// Set active state
pub struct SetActive {
  pub active: bool,
}

impl Message for SetActive {
  type Result = ();
}

#[async_trait]
impl Handler<SetActive> for PingCollectActor {
  async fn handle(
    &mut self,
    _: &mut Context<Self>,
    SetActive { active }: SetActive,
  ) -> <SetActive as Message>::Result {
    if self.is_active != active {
      tracing::debug!(
        address = self.address_str(),
        active = active,
        "Setting active state: {}",
        active
      );
      self.is_active = active;
    }
  }
}

#[async_trait]
impl Handler<PingError> for PingCollectActor {
  async fn handle(
    &mut self,
    ctx: &mut Context<Self>,
    message: PingError,
  ) -> <PingError as Message>::Result {
    tracing::error!(address = self.address_str(), error = ?message, "Ping error received");
    self.add_result(None, ctx);
  }
}

// Add new message for starting warmup
struct StartWarmup;
impl Message for StartWarmup {
  type Result = ();
}

#[async_trait]
impl Handler<StartWarmup> for PingCollectActor {
  async fn handle(
    &mut self,
    ctx: &mut Context<Self>,
    _: StartWarmup,
  ) -> <StartWarmup as Message>::Result {
    self.start_warmup_task(ctx);
  }
}

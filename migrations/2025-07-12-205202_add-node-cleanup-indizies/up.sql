-- Composite index for game status and updated_at columns
-- Optimizes queries in get_expired_games that filter by status and updated_at
-- Covers both game creation timeout and game play timeout queries
CREATE INDEX idx_game_status_updated_at ON game(status, updated_at);

-- Composite index for game_used_slot player_id, client_status, and updated_at columns  
-- Optimizes the slot timeout query in get_expired_games that filters by:
-- - player_id IS NOT NULL
-- - client_status != SlotClientStatus::Left (4)
-- - updated_at < timeout_threshold
CREATE INDEX idx_game_used_slot_player_client_updated ON game_used_slot(player_id, client_status, updated_at);

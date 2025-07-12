-- This file should undo anything in `up.sql`

-- Drop indexes in reverse order of creation
DROP INDEX IF EXISTS idx_game_used_slot_player_client_updated;
DROP INDEX IF EXISTS idx_game_status_updated_at;

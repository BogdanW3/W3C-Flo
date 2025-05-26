-- Partial index for active bans (ban_expires_at > now() OR ban_expires_at IS NULL)
-- This specifically targets the filter condition in get_ban_list_map
CREATE INDEX idx_player_ban_active ON player_ban(player_id, ban_type, ban_expires_at)
WHERE ban_expires_at IS NULL;

-- Additional index for the case where ban_expires_at has a value
-- The query planner can use this for the condition where ban_expires_at > current timestamp
CREATE INDEX idx_player_ban_with_expiry ON player_ban(player_id, ban_type, ban_expires_at);

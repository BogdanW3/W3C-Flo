-- Revert unique constraint back to (player_id, ban_type)

ALTER TABLE player_ban DROP CONSTRAINT player_ban_unique_ban;

ALTER TABLE player_ban ADD CONSTRAINT player_ban_player_id_ban_type_key 
UNIQUE (player_id, ban_type);

ALTER TABLE player_ban DROP CONSTRAINT player_ban_player_id_ban_type_key;

ALTER TABLE player_ban ADD CONSTRAINT player_ban_unique_ban 
UNIQUE (player_id, ban_type, ban_expires_at);
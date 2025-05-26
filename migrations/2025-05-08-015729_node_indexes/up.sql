-- Composite index for ordering by location and name (get_all_nodes uses this)
CREATE INDEX idx_node_location_name ON node(location, name);

-- Partial index for active nodes (get_all_nodes uses this)
CREATE INDEX idx_node_active ON node(location, name) 
WHERE disabled = false;

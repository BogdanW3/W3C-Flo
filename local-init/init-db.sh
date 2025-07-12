#!/bin/bash

while ! pg_isready -h $POSTGRES_HOST -p $POSTGRES_PORT; do
  echo "Waiting for postgres"
  sleep 0.5
done

/usr/local/cargo/bin/diesel --config-file /usr/local/flo/diesel.toml migration run

# Now create the default API client, players and node
PGPASSWORD=$POSTGRES_PASSWORD psql -h $POSTGRES_HOST -p $POSTGRES_PORT -d $POSTGRES_DB -U $POSTGRES_USER <<EOF
insert into api_client (name, secret_key)
select 'testclient', '1111'
where not exists (
    select 1 from api_client where name = 'testclient'
);

insert into player (name, source, source_id, api_client_id)
select 'player1', '0', '1', '1'
where not exists (
    select 1 from player where name = 'player1' and source = '0' and source_id = '1' and api_client_id = '1'
);

insert into player (name, source, source_id, api_client_id)
select 'player2', '0', '2', '1'
where not exists (
    select 1 from player where name = 'player2' and source = '0' and source_id = '2' and api_client_id = '1'
);

insert into node (name, location, ip_addr, secret, internal_address)
select 'node $NODE_EXTERNAL_IP', 'Germany', '$NODE_EXTERNAL_IP', '1111', 'flo-node'
where not exists (
    select 1 from node where name = 'node $NODE_EXTERNAL_IP' and location = 'Germany' and ip_addr = '$NODE_EXTERNAL_IP'
);

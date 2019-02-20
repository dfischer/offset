use std::path::PathBuf;
use std::collections::HashMap;

use futures::executor::ThreadPool;
use futures::task::{Spawn, SpawnExt};
use futures::{FutureExt, TryFutureExt};

use crypto::identity::{PublicKey, Identity, 
    generate_pkcs8_key_pair};

use proto::consts::{TICKS_TO_REKEY, MAX_OPERATIONS_IN_BATCH, 
    MAX_NODE_RELAYS, KEEPALIVE_TICKS};
use proto::net::messages::NetAddress;
use proto::app_server::messages::AppPermissions;

use identity::{IdentityClient, create_identity};

use node::connect::node_connect;
use node::{net_node, NetNodeError, NodeConfig, NodeState};

use database::AtomicDb;
use database::file_db::FileDb;

use relay::{net_relay_server, NetRelayServerError};
use index_server::{net_index_server, NetIndexServerError};

use timer::{TimerClient, create_timer_incoming, TimerTick};

use crate::sim_network::{SimNetworkClient, create_sim_network};

/// Memory allocated to a channel in memory (Used to connect two components)
const CHANNEL_LEN: usize = 0x20;
/// The amount of ticks we wait before attempting to reconnect
const BACKOFF_TICKS: usize = 0x8;
/// Maximum amount of encryption set ups (diffie hellman) that we allow to occur at the same
/// time.
const MAX_CONCURRENT_ENCRYPT: usize = 0x8;
/// The size we allocate for the user send funds requests queue.
const MAX_PENDING_USER_REQUESTS: usize = 0x20;
/// Maximum amount of concurrent index client requests:
const MAX_OPEN_INDEX_CLIENT_REQUESTS: usize = 0x8;
/// The amount of ticks we are willing to wait until a connection is established (Through
/// the relay)
const CONN_TIMEOUT_TICKS: usize = 0x8;
/// Maximum amount of concurrent applications
/// going through the incoming connection transform at the same time
const MAX_CONCURRENT_INCOMING_APPS: usize = 0x8;

fn gen_identity(seed: &[u8]) -> impl Identity {
    let rng = DummyRandom::new(seed);
    let pkcs8 = generate_pkcs8_key_pair(&rng);
    SoftwareEd25519Identity::from_pkcs8(&pkcs8).unwrap()
}


fn create_identity_client<S>(seed: &[u8], 
                             mut spawner: S) -> IdentityClient
where
    S: Spawn,
{
    let identity = gen_identity(seed);
    let (requests_sender, identity_server) = create_identity(identity);
    let identity_client = IdentityClient::new(requests_sender);
    spawner.spawn(identity_server.then(|_| future::ready(()))).unwrap();
    identity_client
}

fn default_node_config() -> NodeConfig {
    NodeConfig {
        /// Memory allocated to a channel in memory (Used to connect two components)
        channel_len: CHANNEL_LEN,
        /// The amount of ticks we wait before attempting to reconnect
        backoff_ticks: BACKOFF_TICKS,
        /// The amount of ticks we wait until we decide an idle connection has timed out.
        keepalive_ticks: KEEPALIVE_TICKS,
        /// Amount of ticks to wait until the next rekeying (Channel encryption)
        ticks_to_rekey: TICKS_TO_REKEY,
        /// Maximum amount of encryption set ups (diffie hellman) that we allow to occur at the same
        /// time.
        max_concurrent_encrypt: MAX_CONCURRENT_ENCRYPT,
        /// The amount of ticks we are willing to wait until a connection is established (Through
        /// the relay)
        conn_timeout_ticks: CONN_TIMEOUT_TICKS,
        /// Maximum amount of operations in one move token message
        max_operations_in_batch: MAX_OPERATIONS_IN_BATCH,
        /// The size we allocate for the user send funds requests queue.
        max_pending_user_requests: MAX_PENDING_USER_REQUESTS,
        /// Maximum amount of concurrent index client requests:
        max_open_index_client_requests: MAX_OPEN_INDEX_CLIENT_REQUESTS,
        /// Maximum amount of relays a node may use.
        max_node_relays: MAX_NODE_RELAYS,
        /// Maximum amount of incoming app connectinos we set up at the same time
        max_concurrent_incoming_apps: MAX_CONCURRENT_INCOMING_APPS,
    }
}

#[derive(Clone)]
struct SimDb {
    temp_dir_path: PathBuf,
}

impl SimDb {
    /// Create an empty node database
    fn init_db(index: u8) -> FileDb {
        let identity = gen_identity([index]);
        let local_public_key = identity.get_public_key();

        // Create a new database file:
        let db_path_buf = self.temp_dir_path.join(format!("db_{}",index));
        let initial_state = NodeState::<NetAddress>::new(local_public_key);
        let _ = FileDb::create(db_path_buf, initial_state)
            .map_err(|_| InitNodeDbError::FileDbError)?;
    }

    /// Load a database. The database should already exist,
    /// otherwise a panic happens.
    fn load_db(&self, index: u8) -> FileDb {
        let db_path_buf = self.temp_dir_path.join(format!("db_{}",index));

        // Load database from file:
        FileDb::<NodeState<NetAddress>>::load(db_path_buf).unwrap();
    }
}

async fn create_node<S>(index: u8, 
              sim_db: SimDb,
              timer_client: TimerClient,
              sim_network_client: SimNetworkClient,
              trusted_apps: HashMap<PublicKey, AppPermissions>,
              spawner: S) 
where
    S: Spawn + Send + Sync + Clone,
{ 

    let rng = DummyRandom::new(&[0x13, 0x37, index]);
    let identity_client = create_identity_client(&[index], spawner);
    let listen_address = format!("node_address_{}", index);
    let incoming_app_raw_conns = await!(sim_network_client.listen(listen_address)).unwrap();
    let get_trusted_apps = || Some(trusted_apps);

    let net_node_fut = net_node(incoming_app_raw_conns,
             sim_network_client,
             timer_client,
             identity_client,
             rng,
             default_node_config(),
             get_trusted_apps,
             sim_db.load_db(index),
             spawner)
        .map_err(|e| error!("net_node() error: {:?}", e))
        .map(|_| ());

    spawner.spawn(net_node_fut).unwrap();
}


async fn task_basic<S>(spawner: S) 
where
    S: Spawn,
{
    let sim_client = create_sim_network(&mut spawner);

    /*
    let (tick_sender, tick_receiver) = mpsc::channel(0);
    let timer_client = create_timer_incoming(tick_receiver, spawner.clone()).unwrap();
    */

}

#[test]
fn test_basic() {
    let mut thread_pool = ThreadPool::new().unwrap();
    thread_pool.run(task_sim_network_basic(thread_pool.clone()));
}

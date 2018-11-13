use super::*;

use std::mem;

use futures::executor::ThreadPool;
use futures::{future, FutureExt};
use futures::task::SpawnExt;

use identity::{create_identity, IdentityClient};

use crypto::test_utils::DummyRandom;
use crypto::identity::{SoftwareEd25519Identity, generate_pkcs8_key_pair};
use crypto::crypto_rand::{RngContainer, CryptoRandom};

use crate::token_channel::{is_public_key_lower};

use crate::types::{FunderIncoming, IncomingControlMessage, 
    AddFriend, IncomingLivenessMessage, FriendStatus,
    SetFriendStatus, FriendMessage, SetFriendRemoteMaxDebt};


/// A helper function. Applies an incoming funder message, updating state and ephemeral
/// accordingly:
async fn apply_funder_incoming<'a,A: Clone + 'static,R: CryptoRandom + 'static>(funder_incoming: FunderIncoming<A>,
                               state: &'a mut FunderState<A>, 
                               ephemeral: &'a mut FunderEphemeral, 
                               rng: R, 
                               identity_client: IdentityClient) 
                -> Result<(Vec<FunderOutgoingComm<A>>, Vec<FunderOutgoingControl<A>>), FunderHandlerError> {

    let funder_handler_output = await!(funder_handle_message(identity_client,
                          rng,
                          state.clone(),
                          ephemeral.clone(),
                          funder_incoming))?;

    let FunderHandlerOutput {ephemeral: new_ephemeral, mutations, outgoing_comms, outgoing_control}
        = funder_handler_output;
    let _ = mem::replace(ephemeral, new_ephemeral);

    // Mutate the state according to the mutations:
    for mutation in &mutations {
        state.mutate(mutation);
    }
    Ok((outgoing_comms, outgoing_control))
}

async fn task_handler_pair_basic(identity_client1: IdentityClient, 
                                 identity_client2: IdentityClient) {
    // Sort the identities. identity_client1 will be the first sender:
    let pk1 = await!(identity_client1.request_public_key()).unwrap();
    let pk2 = await!(identity_client2.request_public_key()).unwrap();
    let (identity_client1, pk1, identity_client2, pk2) = if is_public_key_lower(&pk1, &pk2) {
        (identity_client1, pk1, identity_client2, pk2)
    } else {
        (identity_client2, pk2, identity_client1, pk1)
    };

    let mut state1 = FunderState::<u32>::new(&pk1);
    let mut ephemeral1 = FunderEphemeral::new(&state1);
    let mut state2 = FunderState::<u32>::new(&pk2);
    let mut ephemeral2 = FunderEphemeral::new(&state2);

    let rng = RngContainer::new(DummyRandom::new(&[3u8]));

    // Initialize 1:
    let funder_incoming = FunderIncoming::Init;
    await!(apply_funder_incoming(funder_incoming, &mut state1, &mut ephemeral1, 
                                 rng.clone(), identity_client1.clone())).unwrap();

    // Initialize 2:
    let funder_incoming = FunderIncoming::Init;
    await!(apply_funder_incoming(funder_incoming, &mut state2, &mut ephemeral2, 
                                 rng.clone(), identity_client2.clone())).unwrap();

    // Node1: Add friend 2:
    let add_friend = AddFriend {
        friend_public_key: pk2.clone(),
        address: 22u32,
    };
    let incoming_control_message = IncomingControlMessage::AddFriend(add_friend);
    let funder_incoming = FunderIncoming::Control(incoming_control_message);
    await!(apply_funder_incoming(funder_incoming, &mut state1, &mut ephemeral1, 
                                 rng.clone(), identity_client1.clone())).unwrap();

    // Node1: Enable friend 2:
    let set_friend_status = SetFriendStatus {
        friend_public_key: pk2.clone(),
        status: FriendStatus::Enable,
    };
    let incoming_control_message = IncomingControlMessage::SetFriendStatus(set_friend_status);
    let funder_incoming = FunderIncoming::Control(incoming_control_message);
    await!(apply_funder_incoming(funder_incoming, &mut state1, &mut ephemeral1, 
                                 rng.clone(), identity_client1.clone())).unwrap();

    // Node2: Add friend 1:
    let add_friend = AddFriend {
        friend_public_key: pk1.clone(),
        address: 11u32,
    };
    let incoming_control_message = IncomingControlMessage::AddFriend(add_friend);
    let funder_incoming = FunderIncoming::Control(incoming_control_message);
    await!(apply_funder_incoming(funder_incoming, &mut state2, &mut ephemeral2, 
                                 rng.clone(), identity_client2.clone())).unwrap();


    // Node2: enable friend 1:
    let set_friend_status = SetFriendStatus {
        friend_public_key: pk1.clone(),
        status: FriendStatus::Enable,
    };
    let incoming_control_message = IncomingControlMessage::SetFriendStatus(set_friend_status);
    let funder_incoming = FunderIncoming::Control(incoming_control_message);
    await!(apply_funder_incoming(funder_incoming, &mut state2, &mut ephemeral2, 
                                 rng.clone(), identity_client2.clone())).unwrap();


    // Node1: Notify that Node2 is alive
    // We expect that Node1 will resend his outgoing message when he is notified that Node1 is online.
    let incoming_liveness_message = IncomingLivenessMessage::Online(pk2.clone());
    let funder_incoming = FunderIncoming::Comm(IncomingCommMessage::Liveness(incoming_liveness_message));
    let (outgoing_comms, _outgoing_control) = await!(apply_funder_incoming(funder_incoming, &mut state1, &mut ephemeral1, 
                                 rng.clone(), identity_client1.clone())).unwrap();

    assert_eq!(outgoing_comms.len(), 1);
    let friend_message = match &outgoing_comms[0] {
        FunderOutgoingComm::FriendMessage((pk, friend_message)) => {
            if let FriendMessage::MoveTokenRequest(move_token_request) = friend_message {
                assert_eq!(pk, &pk2);
                assert_eq!(move_token_request.token_wanted, false);

                let friend_move_token = &move_token_request.friend_move_token;
                assert_eq!(friend_move_token.move_token_counter, 0);
                assert_eq!(friend_move_token.inconsistency_counter, 0);
                assert_eq!(friend_move_token.balance, 0);

            } else {
                unreachable!();
            }
            friend_message.clone()
        },
        _ => unreachable!(),
    };

    // Node2: Notify that Node1 is alive
    let incoming_liveness_message = IncomingLivenessMessage::Online(pk1.clone());
    let funder_incoming = FunderIncoming::Comm(IncomingCommMessage::Liveness(incoming_liveness_message));
    await!(apply_funder_incoming(funder_incoming, &mut state2, &mut ephemeral2, 
                                 rng.clone(), identity_client2.clone())).unwrap();

    // Node2: Receive friend_message from Node1:
    let funder_incoming = FunderIncoming::Comm(IncomingCommMessage::Friend((pk1.clone(), friend_message)));
    await!(apply_funder_incoming(funder_incoming, &mut state2, &mut ephemeral2, 
                                 rng.clone(), identity_client2.clone())).unwrap();

    println!("Node1 receives control message to set remote max debt:");

    // TODO:
    // Node1 receives control message to set remote max debt:
    let set_friend_remote_max_debt = SetFriendRemoteMaxDebt {
        friend_public_key: pk2.clone(),
        remote_max_debt: 100,
    };
    let incoming_control_message = IncomingControlMessage::SetFriendRemoteMaxDebt(set_friend_remote_max_debt);
    let funder_incoming = FunderIncoming::Control(incoming_control_message);
    let (outgoing_comms, _outgoing_control) = await!(apply_funder_incoming(funder_incoming, &mut state1, &mut ephemeral1, 
                                 rng.clone(), identity_client1.clone())).unwrap();
    // Node1 wants to obtain the token, so it resends the last outgoing move token with
    // token_wanted = true:
    assert_eq!(outgoing_comms.len(), 1);
    let friend_message = match &outgoing_comms[0] {
        FunderOutgoingComm::FriendMessage((pk, friend_message)) => {
            if let FriendMessage::MoveTokenRequest(move_token_request) = friend_message {
                assert_eq!(pk, &pk2);
                assert_eq!(move_token_request.token_wanted, true);
            } else {
                unreachable!();
            }
            friend_message.clone()
        },
        _ => unreachable!(),
    };

    // Node2: Receive friend_message (request for token) from Node1:
    let funder_incoming = FunderIncoming::Comm(IncomingCommMessage::Friend((pk1.clone(), friend_message)));
    let (outgoing_comms, _outgoing_control) = await!(apply_funder_incoming(funder_incoming, &mut state2, &mut ephemeral2, 
                                 rng.clone(), identity_client2.clone())).unwrap();
    assert_eq!(outgoing_comms.len(), 1);
    let friend_message = match &outgoing_comms[0] {
        FunderOutgoingComm::FriendMessage((pk, friend_message)) => {
            if let FriendMessage::MoveTokenRequest(_move_token_request) = friend_message {
                assert_eq!(pk, &pk1);
            } else {
                unreachable!();
            }
            friend_message.clone()
        },
        _ => unreachable!(),
    };

    // Node1: Receive friend_message from Node2:
    let funder_incoming = FunderIncoming::Comm(IncomingCommMessage::Friend((pk2.clone(), friend_message)));
    let (outgoing_comms, _outgoing_control) = await!(apply_funder_incoming(funder_incoming, &mut state1, &mut ephemeral1, 
                                 rng.clone(), identity_client1.clone())).unwrap();

    // Now that Node1 has the token, it will now send the SetRemoteMaxDebt message to Node2:
    assert_eq!(outgoing_comms.len(), 1);
    let friend_message = match &outgoing_comms[0] {
        FunderOutgoingComm::FriendMessage((pk, friend_message)) => {
            if let FriendMessage::MoveTokenRequest(move_token_request) = friend_message {
                assert_eq!(pk, &pk2);
                assert_eq!(move_token_request.token_wanted, false);
            } else {
                unreachable!();
            }
            friend_message.clone()
        },
        _ => unreachable!(),
    };

    // Node2: Receive friend_message from Node1:
    let funder_incoming = FunderIncoming::Comm(IncomingCommMessage::Friend((pk1.clone(), friend_message)));
    let (_outgoing_comms, _outgoing_control) = await!(apply_funder_incoming(funder_incoming, &mut state2, &mut ephemeral2, 
                                 rng.clone(), identity_client2.clone())).unwrap();

    /*
    // TODO:
    // Node1 receives control message to send credit
    let user_request_send_funds = UserRequestSendFunds {
        request_id: Uid::from(&[3; UID_LEN]),
        route: FriendsRoute { public_keys: vec![pk1.clone(), pk2.clone()] },
        invoice_id: InvoiceId::from(&[0; INVOICE_ID_LEN]),
        dest_payment: 20,
    };
    let incoming_control_message = IncomingControlMessage::RequestSendFunds(user_request_send_funds);
    let funder_incoming = FunderIncoming::Control(incoming_control_message);
    await!(apply_funder_incoming(funder_incoming, &mut state1, &mut ephemeral1, 
                                 rng.clone(), identity_client1.clone())).unwrap();
     */

    // Node1 sends a message to Node2, requesting for the token.
    // Node2 sends back and empty message, containing the token.
    // Node1 sends a request to Node2.
    // Node2 sends a response to Node1.
    // Both state1 and state2 reflect the moving of the funds.
}

#[test]
fn test_handler_pair_basic() {
    let mut thread_pool = ThreadPool::new().unwrap();

    let rng1 = DummyRandom::new(&[1u8]);
    let pkcs8 = generate_pkcs8_key_pair(&rng1);
    let identity1 = SoftwareEd25519Identity::from_pkcs8(&pkcs8).unwrap();
    let (requests_sender1, identity_server1) = create_identity(identity1);
    let identity_client1 = IdentityClient::new(requests_sender1);
    thread_pool.spawn(identity_server1.then(|_| future::ready(()))).unwrap();

    let rng2 = DummyRandom::new(&[2u8]);
    let pkcs8 = generate_pkcs8_key_pair(&rng2);
    let identity2 = SoftwareEd25519Identity::from_pkcs8(&pkcs8).unwrap();
    let (requests_sender2, identity_server2) = create_identity(identity2);
    let identity_client2 = IdentityClient::new(requests_sender2);
    thread_pool.spawn(identity_server2.then(|_| future::ready(()))).unwrap();

    thread_pool.run(task_handler_pair_basic(identity_client1, identity_client2));
}

// TODO: Add a test about inconsistency and resolving it.

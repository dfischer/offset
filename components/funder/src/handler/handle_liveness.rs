use proto::funder::messages::{FriendStatus, FunderOutgoingControl};
use proto::funder::scheme::FunderScheme;

use crate::types::{IncomingLivenessMessage};

use crate::ephemeral::EphemeralMutation;
use crate::liveness::LivenessMutation;

use crate::handler::handler::{MutableFunderState, MutableEphemeral};
use crate::handler::sender::SendCommands;
use crate::handler::canceler::{cancel_pending_requests, 
                                cancel_pending_user_requests};

#[derive(Debug)]
pub enum HandleLivenessError {
    FriendDoesNotExist,
    FriendIsDisabled,
    FriendAlreadyOnline,
}

pub fn handle_liveness_message<FS>(m_state: &mut MutableFunderState<FS>,
                                    m_ephemeral: &mut MutableEphemeral,
                                    send_commands: &mut SendCommands,
                                    outgoing_control: &mut Vec<FunderOutgoingControl<FS>>,
                                    liveness_message: IncomingLivenessMessage)
    -> Result<(), HandleLivenessError> 

where
    FS: FunderScheme,
{

    match liveness_message {
        IncomingLivenessMessage::Online(friend_public_key) => {
            // Find friend:
            let friend = match m_state.state().friends.get(&friend_public_key) {
                Some(friend) => Ok(friend),
                None => Err(HandleLivenessError::FriendDoesNotExist),
            }?;
            match friend.status {
                FriendStatus::Enabled => Ok(()),
                FriendStatus::Disabled => Err(HandleLivenessError::FriendIsDisabled),
            }?;

            if m_ephemeral.ephemeral().liveness.is_online(&friend_public_key) {
                return Err(HandleLivenessError::FriendAlreadyOnline);
            }

            send_commands.set_resend_outgoing(&friend_public_key);

            let liveness_mutation = LivenessMutation::SetOnline(friend_public_key.clone());
            let ephemeral_mutation = EphemeralMutation::LivenessMutation(liveness_mutation);
            m_ephemeral.mutate(ephemeral_mutation);
        },
        IncomingLivenessMessage::Offline(friend_public_key) => {
            // Find friend:
            let friend = match m_state.state().friends.get(&friend_public_key) {
                Some(friend) => Ok(friend),
                None => Err(HandleLivenessError::FriendDoesNotExist),
            }?;
            match friend.status {
                FriendStatus::Enabled => Ok(()),
                FriendStatus::Disabled => Err(HandleLivenessError::FriendIsDisabled),
            }?;
            let liveness_mutation = LivenessMutation::SetOffline(friend_public_key.clone());
            let ephemeral_mutation = EphemeralMutation::LivenessMutation(liveness_mutation);
            m_ephemeral.mutate(ephemeral_mutation);

            // Cancel all messages pending for this friend:
            cancel_pending_requests(
                m_state, 
                send_commands,
                outgoing_control,
                &friend_public_key);
            cancel_pending_user_requests(
                m_state,
                outgoing_control,
                &friend_public_key);
        },
    };
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;

    use std::cmp::Ordering;

    use proto::funder::messages::{FriendStatus, AddFriend};
    use crypto::test_utils::DummyRandom;
    use crypto::identity::{SoftwareEd25519Identity,
                            generate_pkcs8_key_pair, compare_public_key,
                            Identity};

    use crate::state::{FunderState, FunderMutation};
    use crate::ephemeral::Ephemeral;
    use crate::friend::{FriendMutation, ChannelStatus};

    use crate::handler::handler::{MutableFunderState, MutableEphemeral};
    use crate::handler::sender::{SendCommands};

    use crate::test_scheme::TestFunderScheme;


    #[test]
    fn test_handle_liveness_basic() {

        let rng1 = DummyRandom::new(&[1u8]);
        let pkcs8 = generate_pkcs8_key_pair(&rng1);
        let identity1 = SoftwareEd25519Identity::from_pkcs8(&pkcs8).unwrap();

        let rng2 = DummyRandom::new(&[2u8]);
        let pkcs8 = generate_pkcs8_key_pair(&rng2);
        let identity2 = SoftwareEd25519Identity::from_pkcs8(&pkcs8).unwrap();

        let pk1 = identity1.get_public_key();
        let pk2 = identity2.get_public_key();

        let (_local_identity, local_pk, _remote_identity, remote_pk) = if compare_public_key(&pk1, &pk2) == Ordering::Less {
            (identity1, pk1, identity2, pk2)
        } else {
            (identity2, pk2, identity1, pk1)
        };

        let mut state = FunderState::<TestFunderScheme>::new(&local_pk, &("1337".to_string(), 1337u32));
        // Add a remote friend:
        let add_friend = AddFriend {
            friend_public_key: remote_pk.clone(),
            address: 3u32,
            name: "remote_pk".into(),
            balance: 0i128,
        };
        let funder_mutation = FunderMutation::AddFriend(add_friend);
        state.mutate(&funder_mutation);

        // Enable the remote friend:
        let friend_mutation = FriendMutation::SetStatus(FriendStatus::Enabled);
        let funder_mutation = FunderMutation::FriendMutation((remote_pk.clone(), friend_mutation));
        state.mutate(&funder_mutation);

        // Make sure that our side of the token channel is outgoing:
        let friend = state.friends.get(&remote_pk).unwrap();
        let token_channel = match &friend.channel_status {
            ChannelStatus::Consistent(token_channel) => token_channel,
            _ => unreachable!(),
        };
        assert!(token_channel.is_outgoing());

        let ephemeral = Ephemeral::new();

        let mut m_state = MutableFunderState::new(state);
        let mut m_ephemeral = MutableEphemeral::new(ephemeral);
        let mut send_commands = SendCommands::new();
        let mut outgoing_control = Vec::new();
        let liveness_message = IncomingLivenessMessage::Online(remote_pk.clone());

        // Remote side got online:
        handle_liveness_message(&mut m_state,
                                &mut m_ephemeral,
                                &mut send_commands,
                                &mut outgoing_control,
                                liveness_message).unwrap();

        let (_initial_state, funder_mutations, _final_state) = m_state.done();
        let (ephemeral_mutations, final_ephemeral_state) = m_ephemeral.done();

        assert!(outgoing_control.is_empty());
        assert!(funder_mutations.is_empty());
        assert_eq!(ephemeral_mutations.len(), 1);
        assert!(final_ephemeral_state.liveness.is_online(&remote_pk));

        // We expect that the local side will send the remote side a message:
        let friend_send_commands = send_commands.send_commands.get(&remote_pk).unwrap();
        assert!(friend_send_commands.resend_outgoing);
    }
}

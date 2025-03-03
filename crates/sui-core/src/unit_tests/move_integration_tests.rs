// Copyright (c) 2021, Facebook, Inc. and its affiliates
// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::{
    authority::authority_tests::{
        call_move, call_move_with_shared, init_state_with_ids, send_and_confirm_transaction,
        TestCallArg,
    },
    test_utils::to_sender_signed_transaction,
};

use move_core_types::{
    language_storage::TypeTag,
    value::{MoveStruct, MoveValue},
};
use move_package::BuildConfig;
use sui_types::{
    crypto::{get_key_pair, AccountKeyPair},
    event::{Event, EventType, TransferType},
    messages::ExecutionStatus,
    object::OBJECT_START_VERSION,
};

use std::path::PathBuf;
use std::{env, str::FromStr};

const MAX_GAS: u64 = 10000;

#[tokio::test]
async fn test_object_wrapping_unwrapping() {
    let (sender, sender_key): (_, AccountKeyPair) = get_key_pair();
    let gas = ObjectID::random();
    let authority = init_state_with_ids(vec![(sender, gas)]).await;

    let package =
        build_and_publish_test_package(&authority, &sender, &sender_key, &gas, "object_wrapping")
            .await;

    // Create a Child object.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_wrapping",
        "create_child",
        vec![],
        vec![],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let child_object_ref = effects.created[0].0;
    assert_eq!(child_object_ref.1, OBJECT_START_VERSION);

    // Create a Parent object, by wrapping the child object.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_wrapping",
        "create_parent",
        vec![],
        vec![TestCallArg::Object(child_object_ref.0)],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    // Child object is wrapped, Parent object is created.
    assert_eq!(
        (
            effects.created.len(),
            effects.deleted.len(),
            effects.wrapped.len()
        ),
        (1, 0, 1)
    );
    let new_child_object_ref = effects.wrapped[0];
    let expected_child_object_ref = (
        child_object_ref.0,
        child_object_ref.1.increment(),
        ObjectDigest::OBJECT_DIGEST_WRAPPED,
    );
    // Make sure that the child's version gets increased after wrapped.
    assert_eq!(new_child_object_ref, expected_child_object_ref);
    check_latest_object_ref(&authority, &expected_child_object_ref).await;
    let child_object_ref = new_child_object_ref;

    let parent_object_ref = effects.created[0].0;
    assert_eq!(parent_object_ref.1, OBJECT_START_VERSION);

    // Extract the child out of the parent.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_wrapping",
        "extract_child",
        vec![],
        vec![TestCallArg::Object(parent_object_ref.0)],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    // Check that the child shows up in unwrapped, not created.
    // mutated contains parent and gas.
    assert_eq!(
        (
            effects.mutated.len(),
            effects.created.len(),
            effects.unwrapped.len()
        ),
        (2, 0, 1)
    );
    // Make sure that version increments again when unwrapped.
    assert_eq!(effects.unwrapped[0].0 .1, child_object_ref.1.increment());
    check_latest_object_ref(&authority, &effects.unwrapped[0].0).await;
    let child_object_ref = effects.unwrapped[0].0;

    // Wrap the child to the parent again.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_wrapping",
        "set_child",
        vec![],
        vec![
            TestCallArg::Object(parent_object_ref.0),
            TestCallArg::Object(child_object_ref.0),
        ],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    // Check that child object showed up in wrapped.
    // mutated contains parent and gas.
    assert_eq!((effects.mutated.len(), effects.wrapped.len()), (2, 1));
    let expected_child_object_ref = (
        child_object_ref.0,
        child_object_ref.1.increment(),
        ObjectDigest::OBJECT_DIGEST_WRAPPED,
    );
    assert_eq!(effects.wrapped[0], expected_child_object_ref);
    check_latest_object_ref(&authority, &expected_child_object_ref).await;
    let child_object_ref = effects.wrapped[0];
    let parent_object_ref = effects.mutated_excluding_gas().next().unwrap().0;

    // Now delete the parent object, which will in turn delete the child object.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_wrapping",
        "delete_parent",
        vec![],
        vec![TestCallArg::Object(parent_object_ref.0)],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    assert_eq!(effects.deleted.len(), 2);
    // Check that both objects are marked as wrapped in the authority.
    let expected_child_object_ref = (
        child_object_ref.0,
        child_object_ref.1.increment(),
        ObjectDigest::OBJECT_DIGEST_DELETED,
    );
    assert!(effects.deleted.contains(&expected_child_object_ref));
    check_latest_object_ref(&authority, &expected_child_object_ref).await;
    let expected_parent_object_ref = (
        parent_object_ref.0,
        parent_object_ref.1.increment(),
        ObjectDigest::OBJECT_DIGEST_DELETED,
    );
    assert!(effects.deleted.contains(&expected_parent_object_ref));
    check_latest_object_ref(&authority, &expected_parent_object_ref).await;
}

#[tokio::test]
async fn test_object_owning_another_object() {
    let (sender, sender_key): (_, AccountKeyPair) = get_key_pair();
    let gas = ObjectID::random();
    let authority = init_state_with_ids(vec![(sender, gas)]).await;

    let package =
        build_and_publish_test_package(&authority, &sender, &sender_key, &gas, "object_owner")
            .await;

    // Create a parent.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_owner",
        "create_parent",
        vec![],
        vec![],
    )
    .await
    .unwrap();
    assert!(effects.status.is_ok());
    assert_eq!(effects.events.len(), 1);
    assert_eq!(effects.events[0].event_type(), EventType::NewObject);
    let parent = effects.created[0].0;
    assert_eq!(effects.events[0].object_id(), Some(parent.0));

    // Create a child.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_owner",
        "create_child",
        vec![],
        vec![],
    )
    .await
    .unwrap();
    assert!(effects.status.is_ok());
    assert_eq!(effects.events.len(), 1);
    assert_eq!(effects.events[0].event_type(), EventType::NewObject);
    let child = effects.created[0].0;

    // Mutate the child directly should work fine.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_owner",
        "mutate_child",
        vec![],
        vec![TestCallArg::Object(child.0)],
    )
    .await
    .unwrap();
    assert!(effects.status.is_ok());

    // Add the child to the parent.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_owner",
        "add_child",
        vec![],
        vec![TestCallArg::Object(parent.0), TestCallArg::Object(child.0)],
    )
    .await
    .unwrap();
    assert!(effects.status.is_ok());
    let child_effect = effects
        .mutated
        .iter()
        .find(|((id, _, _), _)| id == &child.0)
        .unwrap();
    // Check that the child is now owned by the parent.
    assert_eq!(child_effect.1, parent.0);

    // Mutate the child directly will now fail because we need the parent to authenticate.
    let result = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_owner",
        "mutate_child",
        vec![],
        vec![TestCallArg::Object(child.0)],
    )
    .await;
    assert!(result.is_err());

    // Mutate the child with the parent will succeed.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_owner",
        "mutate_child_with_parent",
        vec![],
        vec![TestCallArg::Object(child.0), TestCallArg::Object(parent.0)],
    )
    .await
    .unwrap();
    assert!(effects.status.is_ok());

    // Create another parent.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_owner",
        "create_parent",
        vec![],
        vec![],
    )
    .await
    .unwrap();
    assert!(effects.status.is_ok());
    assert_eq!(effects.events.len(), 1);
    assert_eq!(effects.events[0].event_type(), EventType::NewObject);
    let new_parent = effects.created[0].0;

    // Transfer the child to the new_parent.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_owner",
        "transfer_child",
        vec![],
        vec![
            TestCallArg::Object(parent.0),
            TestCallArg::Object(child.0),
            TestCallArg::Object(new_parent.0),
        ],
    )
    .await
    .unwrap();
    assert!(effects.status.is_ok());
    assert_eq!(effects.events.len(), 1);
    let event1 = effects.events[0].clone();
    assert_eq!(event1.event_type(), EventType::TransferObject);
    assert_eq!(event1.object_id(), Some(child.0));
    if let Event::TransferObject {
        recipient, type_, ..
    } = event1
    {
        assert_eq!(type_, TransferType::ToObject);
        assert_eq!(recipient, Owner::ObjectOwner(new_parent.0.into()));
    } else {
        panic!("Unexpected event type: {:?}", event1);
    }

    let child_effect = effects
        .mutated
        .iter()
        .find(|((id, _, _), _)| id == &child.0)
        .unwrap();
    // Check that the child is now owned by the new parent.
    assert_eq!(child_effect.1, new_parent.0);

    // Delete the child. This should fail because the child is still owned by a parent,
    // it cannot yet be deleted.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_owner",
        "delete_child",
        vec![],
        vec![
            TestCallArg::Object(child.0),
            TestCallArg::Object(new_parent.0),
        ],
    )
    .await
    .unwrap();
    // we expect this to be and error due to Deleting an Object Owned Object
    assert!(effects.status.is_ok());

    // Create a parent and a child together. This tests the
    // transfer::transfer_to_object_id() API.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_owner",
        "create_parent_and_child",
        vec![],
        vec![],
    )
    .await
    .unwrap();
    assert!(effects.status.is_ok());
    assert_eq!(effects.created.len(), 2);
    // Check that one of them is the parent and the other is the child.
    let (parent, child) = if effects.created[0].1 == sender {
        assert_eq!(effects.created[1].1, effects.created[0].0 .0);
        (effects.created[0].0, effects.created[1].0)
    } else {
        assert!(effects.created[1].1 == sender && effects.created[0].1 == effects.created[1].0 .0);
        (effects.created[1].0, effects.created[0].0)
    };

    // Delete the parent and child altogether.
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "object_owner",
        "delete_parent_and_child",
        vec![],
        vec![TestCallArg::Object(parent.0), TestCallArg::Object(child.0)],
    )
    .await
    .unwrap();
    assert!(effects.status.is_ok());
    // Check that both objects were deleted.
    assert_eq!(effects.deleted.len(), 2);
}

#[tokio::test]
async fn test_entry_point_vector_empty() {
    let (sender, sender_key): (_, AccountKeyPair) = get_key_pair();
    let gas = ObjectID::random();
    let authority = init_state_with_ids(vec![(sender, gas)]).await;

    let package = build_and_publish_test_package(
        &authority,
        &sender,
        &sender_key,
        &gas,
        "entry_point_vector",
    )
    .await;

    // call a function with an empty vector
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "obj_vec_empty",
        vec![],
        vec![TestCallArg::ObjVec(vec![])],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );

    // call a function with an empty vector whose type is generic
    let type_tag =
        TypeTag::from_str(format!("{}::entry_point_vector::Obj", package.0).as_str()).unwrap();
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "type_param_vec_empty",
        vec![type_tag],
        vec![TestCallArg::ObjVec(vec![])],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
}

#[tokio::test]
async fn test_entry_point_vector_primitive() {
    let (sender, sender_key): (_, AccountKeyPair) = get_key_pair();
    let gas = ObjectID::random();
    let authority = init_state_with_ids(vec![(sender, gas)]).await;

    let package = build_and_publish_test_package(
        &authority,
        &sender,
        &sender_key,
        &gas,
        "entry_point_vector",
    )
    .await;

    // just a test call with vector of 2 primitive values and check its length in the entry function
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "prim_vec_len",
        vec![],
        vec![TestCallArg::Pure(
            bcs::to_bytes(&vec![7_u64, 42_u64]).unwrap(),
        )],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
}

#[tokio::test]
async fn test_entry_point_vector() {
    let (sender, sender_key): (_, AccountKeyPair) = get_key_pair();
    let gas = ObjectID::random();
    let authority = init_state_with_ids(vec![(sender, gas)]).await;

    let package = build_and_publish_test_package(
        &authority,
        &sender,
        &sender_key,
        &gas,
        "entry_point_vector",
    )
    .await;

    // mint an owned object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint",
        vec![],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (obj_id, _, _) = effects.created[0].0;
    // call a function with a vector containing one owned object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "obj_vec_destroy",
        vec![],
        vec![TestCallArg::ObjVec(vec![obj_id])],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );

    // mint a parent object and a child object and make sure that parent stored in the vector
    // authenticates the child passed by-value
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint",
        vec![],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (parent_id, _, _) = effects.created[0].0;
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_child",
        vec![],
        vec![
            TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap()),
            TestCallArg::Object(parent_id),
        ],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (child_id, _, _) = effects.created[0].0;
    // call a function with a vector containing the same owned object as another one passed as
    // a reference argument
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "child_access",
        vec![],
        vec![
            TestCallArg::Object(child_id),
            TestCallArg::ObjVec(vec![parent_id]),
        ],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
}

#[tokio::test]
async fn test_entry_point_vector_error() {
    let (sender, sender_key): (_, AccountKeyPair) = get_key_pair();
    let gas = ObjectID::random();
    let authority = init_state_with_ids(vec![(sender, gas)]).await;

    let package = build_and_publish_test_package(
        &authority,
        &sender,
        &sender_key,
        &gas,
        "entry_point_vector",
    )
    .await;

    // mint an owned object of a wrong type
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_another",
        vec![],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (obj_id, _, _) = effects.created[0].0;
    // call a function with a vector containing one owned object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "obj_vec_destroy",
        vec![],
        vec![TestCallArg::ObjVec(vec![obj_id])],
    )
    .await
    .unwrap();
    // should fail as we passed object of the wrong type
    assert!(
        matches!(effects.status, ExecutionStatus::Failure { .. }),
        "{:?}",
        effects.status
    );

    // mint two objects - one of a wrong type and one of the correct type
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_another",
        vec![],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (wrong_obj_id, _, _) = effects.created[0].0;
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint",
        vec![],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (correct_obj_id, _, _) = effects.created[0].0;
    // call a function with a vector containing one owned object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "two_obj_vec_destroy",
        vec![],
        vec![TestCallArg::ObjVec(vec![wrong_obj_id, correct_obj_id])],
    )
    .await
    .unwrap();
    // should fail as we passed object of the wrong type as the first element of the vector
    assert!(
        matches!(effects.status, ExecutionStatus::Failure { .. }),
        "{:?}",
        effects.status
    );

    // mint a shared object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_shared",
        vec![],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (shared_obj_id, _, _) = effects.created[0].0;
    // call a function with a vector containing one shared object
    let effects = call_move_with_shared(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "obj_vec_destroy",
        vec![],
        vec![TestCallArg::ObjVec(vec![shared_obj_id])],
        true, // shared object in arguments
    )
    .await
    .unwrap();
    // should fail as we do not support shared objects in vectors
    assert!(
        matches!(effects.status, ExecutionStatus::Failure { .. }),
        "{:?}",
        effects.status
    );

    // mint an owned object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint",
        vec![],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (obj_id, _, _) = effects.created[0].0;
    // call a function with a vector containing the same owned object as another one passed as
    // argument
    let result = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "same_objects",
        vec![],
        vec![
            TestCallArg::Object(obj_id),
            TestCallArg::ObjVec(vec![obj_id]),
        ],
    )
    .await;
    // should fail as we have the same object passed in vector and as a separate by-value argument
    assert!(
        matches!(
            result.clone().err().unwrap(),
            SuiError::DuplicateObjectRefInput { .. }
        ),
        "{:?}",
        result
    );

    // mint an owned object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint",
        vec![],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (obj_id, _, _) = effects.created[0].0;
    // call a function with a vector containing the same owned object as another one passed as
    // a reference argument
    let result = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "same_objects_ref",
        vec![],
        vec![
            TestCallArg::Object(obj_id),
            TestCallArg::ObjVec(vec![obj_id]),
        ],
    )
    .await;
    // should fail as we have the same object passed in vector and as a separate by-reference argument
    assert!(
        matches!(
            result.clone().err().unwrap(),
            SuiError::DuplicateObjectRefInput { .. }
        ),
        "{:?}",
        result
    );
}

#[tokio::test]
async fn test_entry_point_vector_any() {
    let (sender, sender_key): (_, AccountKeyPair) = get_key_pair();
    let gas = ObjectID::random();
    let authority = init_state_with_ids(vec![(sender, gas)]).await;

    let package = build_and_publish_test_package(
        &authority,
        &sender,
        &sender_key,
        &gas,
        "entry_point_vector",
    )
    .await;

    let any_type_tag =
        TypeTag::from_str(format!("{}::entry_point_vector::Any", package.0).as_str()).unwrap();

    // mint an owned object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_any",
        vec![any_type_tag.clone()],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (obj_id, _, _) = effects.created[0].0;
    // call a function with a vector containing one owned object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "obj_vec_destroy_any",
        vec![any_type_tag.clone()],
        vec![TestCallArg::ObjVec(vec![obj_id])],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );

    // mint a parent object and a child object and make sure that parent stored in the vector
    // authenticates the child passed by-value
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_any",
        vec![any_type_tag.clone()],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (parent_id, _, _) = effects.created[0].0;
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_child_any",
        vec![any_type_tag.clone()],
        vec![
            TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap()),
            TestCallArg::Object(parent_id),
        ],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (child_id, _, _) = effects.created[0].0;
    // call a function with a vector containing the same owned object as another one passed as
    // a reference argument
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "child_access_any",
        vec![any_type_tag],
        vec![
            TestCallArg::Object(child_id),
            TestCallArg::ObjVec(vec![parent_id]),
        ],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
}

#[tokio::test]
async fn test_entry_point_vector_any_error() {
    let (sender, sender_key): (_, AccountKeyPair) = get_key_pair();
    let gas = ObjectID::random();
    let authority = init_state_with_ids(vec![(sender, gas)]).await;

    let package = build_and_publish_test_package(
        &authority,
        &sender,
        &sender_key,
        &gas,
        "entry_point_vector",
    )
    .await;

    let any_type_tag =
        TypeTag::from_str(format!("{}::entry_point_vector::Any", package.0).as_str()).unwrap();

    // mint an owned object of a wrong type
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_another_any",
        vec![any_type_tag.clone()],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (obj_id, _, _) = effects.created[0].0;
    // call a function with a vector containing one owned object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "obj_vec_destroy_any",
        vec![any_type_tag.clone()],
        vec![TestCallArg::ObjVec(vec![obj_id])],
    )
    .await
    .unwrap();
    // should fail as we passed object of the wrong type
    assert!(
        matches!(effects.status, ExecutionStatus::Failure { .. }),
        "{:?}",
        effects.status
    );

    // mint two objects - one of a wrong type and one of the correct type
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_another_any",
        vec![any_type_tag.clone()],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (wrong_obj_id, _, _) = effects.created[0].0;
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_any",
        vec![any_type_tag.clone()],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (correct_obj_id, _, _) = effects.created[0].0;
    // call a function with a vector containing one owned object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "two_obj_vec_destroy_any",
        vec![any_type_tag.clone()],
        vec![TestCallArg::ObjVec(vec![wrong_obj_id, correct_obj_id])],
    )
    .await
    .unwrap();
    // should fail as we passed object of the wrong type as the first element of the vector
    assert!(
        matches!(effects.status, ExecutionStatus::Failure { .. }),
        "{:?}",
        effects.status
    );

    // mint a shared object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_shared_any",
        vec![any_type_tag.clone()],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (shared_obj_id, _, _) = effects.created[0].0;
    // call a function with a vector containing one shared object
    let effects = call_move_with_shared(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "obj_vec_destroy_any",
        vec![any_type_tag.clone()],
        vec![TestCallArg::ObjVec(vec![shared_obj_id])],
        true, // shared object in arguments
    )
    .await
    .unwrap();
    // should fail as we do not support shared objects in vectors
    assert!(
        matches!(effects.status, ExecutionStatus::Failure { .. }),
        "{:?}",
        effects.status
    );

    // mint an owned object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_any",
        vec![any_type_tag.clone()],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (obj_id, _, _) = effects.created[0].0;
    // call a function with a vector containing the same owned object as another one passed as
    // argument
    let result = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "same_objects_any",
        vec![any_type_tag.clone()],
        vec![
            TestCallArg::Object(obj_id),
            TestCallArg::ObjVec(vec![obj_id]),
        ],
    )
    .await;
    // should fail as we have the same object passed in vector and as a separate by-value argument
    assert!(
        matches!(
            result.clone().err().unwrap(),
            SuiError::DuplicateObjectRefInput { .. }
        ),
        "{:?}",
        result
    );

    // mint an owned object
    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "mint_any",
        vec![any_type_tag.clone()],
        vec![TestCallArg::Pure(bcs::to_bytes(&(42_u64)).unwrap())],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    let (obj_id, _, _) = effects.created[0].0;
    // call a function with a vector containing the same owned object as another one passed as
    // a reference argument
    let result = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_vector",
        "same_objects_ref_any",
        vec![any_type_tag.clone()],
        vec![
            TestCallArg::Object(obj_id),
            TestCallArg::ObjVec(vec![obj_id]),
        ],
    )
    .await;
    assert!(
        matches!(
            result.clone().err().unwrap(),
            SuiError::DuplicateObjectRefInput { .. }
        ),
        "{:?}",
        result
    );
}

#[tokio::test]
async fn test_entry_point_string() {
    let (sender, sender_key): (_, AccountKeyPair) = get_key_pair();
    let gas = ObjectID::random();
    let authority = init_state_with_ids(vec![(sender, gas)]).await;

    let package = build_and_publish_test_package(
        &authority,
        &sender,
        &sender_key,
        &gas,
        "entry_point_string",
    )
    .await;

    // pass a valid ascii string
    let ascii_str = "SomeString";
    let ascii_str_bcs = MoveValue::Struct(MoveStruct::Runtime(vec![MoveValue::Vector(
        ascii_str
            .as_bytes()
            .iter()
            .map(|c| MoveValue::U8(*c))
            .collect(),
    )]))
    .simple_serialize()
    .unwrap();

    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_string",
        "ascii_arg",
        vec![],
        vec![TestCallArg::Pure(ascii_str_bcs)],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );

    // pass a valid utf8 string
    let utf8_str = "çå∞≠¢õß∂ƒ∫";
    let utf_str_bcs = MoveValue::Struct(MoveStruct::Runtime(vec![MoveValue::Vector(
        utf8_str
            .as_bytes()
            .iter()
            .map(|c| MoveValue::U8(*c))
            .collect(),
    )]))
    .simple_serialize()
    .unwrap();

    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_string",
        "utf8_arg",
        vec![],
        vec![TestCallArg::Pure(utf_str_bcs)],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
}

#[tokio::test]
async fn test_entry_point_string_vec() {
    let (sender, sender_key): (_, AccountKeyPair) = get_key_pair();
    let gas = ObjectID::random();
    let authority = init_state_with_ids(vec![(sender, gas)]).await;

    let package = build_and_publish_test_package(
        &authority,
        &sender,
        &sender_key,
        &gas,
        "entry_point_string",
    )
    .await;

    // pass a valid utf8 string vector
    let utf8_str_1 = "çå∞≠¢";
    let utf8_str_2 = "õß∂ƒ∫";
    let utf_str_vec_bcs = MoveValue::Vector(vec![
        MoveValue::Struct(MoveStruct::Runtime(vec![MoveValue::Vector(
            utf8_str_1
                .as_bytes()
                .iter()
                .map(|c| MoveValue::U8(*c))
                .collect(),
        )])),
        MoveValue::Struct(MoveStruct::Runtime(vec![MoveValue::Vector(
            utf8_str_2
                .as_bytes()
                .iter()
                .map(|c| MoveValue::U8(*c))
                .collect(),
        )])),
    ])
    .simple_serialize()
    .unwrap();

    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_string",
        "utf8_vec_arg",
        vec![],
        vec![TestCallArg::Pure(utf_str_vec_bcs)],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
}

#[tokio::test]
async fn test_entry_point_string_error() {
    let (sender, sender_key): (_, AccountKeyPair) = get_key_pair();
    let gas = ObjectID::random();
    let authority = init_state_with_ids(vec![(sender, gas)]).await;

    let package = build_and_publish_test_package(
        &authority,
        &sender,
        &sender_key,
        &gas,
        "entry_point_string",
    )
    .await;

    // pass a invalid ascii string
    let ascii_str = "SomeString";
    let mut ascii_u8_vec: Vec<MoveValue> = ascii_str
        .as_bytes()
        .iter()
        .map(|c| MoveValue::U8(*c))
        .collect();
    // mess up one element
    ascii_u8_vec[7] = MoveValue::U8(255);

    let ascii_str_bcs =
        MoveValue::Struct(MoveStruct::Runtime(vec![MoveValue::Vector(ascii_u8_vec)]))
            .simple_serialize()
            .unwrap();

    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_string",
        "ascii_arg",
        vec![],
        vec![TestCallArg::Pure(ascii_str_bcs)],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Failure { .. }),
        "{:?}",
        effects.status
    );

    // pass a invalid utf8 string
    let utf8_str = "çå∞≠¢õß∂ƒ∫";
    let mut utf8_u8_vec: Vec<MoveValue> = utf8_str
        .as_bytes()
        .iter()
        .map(|c| MoveValue::U8(*c))
        .collect();
    // mess up one element
    utf8_u8_vec[7] = MoveValue::U8(255);

    let utf8_str_bcs = MoveValue::Struct(MoveStruct::Runtime(vec![MoveValue::Vector(utf8_u8_vec)]))
        .simple_serialize()
        .unwrap();

    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_string",
        "utf8_arg",
        vec![],
        vec![TestCallArg::Pure(utf8_str_bcs)],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Failure { .. }),
        "{:?}",
        effects.status
    );
}

#[tokio::test]
async fn test_entry_point_string_vec_error() {
    let (sender, sender_key): (_, AccountKeyPair) = get_key_pair();
    let gas = ObjectID::random();
    let authority = init_state_with_ids(vec![(sender, gas)]).await;

    let package = build_and_publish_test_package(
        &authority,
        &sender,
        &sender_key,
        &gas,
        "entry_point_string",
    )
    .await;

    // pass an utf8 string vector with one invalid string
    let utf8_str_1 = "çå∞≠¢";
    let utf8_str_2 = "õß∂ƒ∫";
    let mut utf8_u8_vec_1: Vec<MoveValue> = utf8_str_1
        .as_bytes()
        .iter()
        .map(|c| MoveValue::U8(*c))
        .collect();
    // mess up one element
    utf8_u8_vec_1[7] = MoveValue::U8(255);
    let utf_str_vec_bcs = MoveValue::Vector(vec![
        MoveValue::Struct(MoveStruct::Runtime(vec![MoveValue::Vector(utf8_u8_vec_1)])),
        MoveValue::Struct(MoveStruct::Runtime(vec![MoveValue::Vector(
            utf8_str_2
                .as_bytes()
                .iter()
                .map(|c| MoveValue::U8(*c))
                .collect(),
        )])),
    ])
    .simple_serialize()
    .unwrap();

    let effects = call_move(
        &authority,
        &gas,
        &sender,
        &sender_key,
        &package,
        "entry_point_string",
        "utf8_vec_arg",
        vec![],
        vec![TestCallArg::Pure(utf_str_vec_bcs)],
    )
    .await
    .unwrap();
    assert!(
        matches!(effects.status, ExecutionStatus::Failure { .. }),
        "{:?}",
        effects.status
    );
}

pub async fn build_and_try_publish_test_package(
    authority: &AuthorityState,
    sender: &SuiAddress,
    sender_key: &AccountKeyPair,
    gas_object_id: &ObjectID,
    test_dir: &str,
    gas_budget: u64,
) -> TransactionInfoResponse {
    let build_config = BuildConfig::default();
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("src/unit_tests/data/");
    path.push(test_dir);
    let modules = sui_framework::build_move_package(&path, build_config).unwrap();

    let all_module_bytes = modules
        .iter()
        .map(|m| {
            let mut module_bytes = Vec::new();
            m.serialize(&mut module_bytes).unwrap();
            module_bytes
        })
        .collect();

    let gas_object = authority.get_object(gas_object_id).await.unwrap();
    let gas_object_ref = gas_object.unwrap().compute_object_reference();

    let data = TransactionData::new_module(*sender, gas_object_ref, all_module_bytes, gas_budget);
    let transaction = to_sender_signed_transaction(data, sender_key);

    send_and_confirm_transaction(authority, transaction)
        .await
        .unwrap()
}

async fn build_and_publish_test_package(
    authority: &AuthorityState,
    sender: &SuiAddress,
    sender_key: &AccountKeyPair,
    gas_object_id: &ObjectID,
    test_dir: &str,
) -> ObjectRef {
    let effects = build_and_try_publish_test_package(
        authority,
        sender,
        sender_key,
        gas_object_id,
        test_dir,
        MAX_GAS,
    )
    .await
    .signed_effects
    .unwrap()
    .effects;
    assert!(
        matches!(effects.status, ExecutionStatus::Success { .. }),
        "{:?}",
        effects.status
    );
    effects.created[0].0
}

async fn check_latest_object_ref(authority: &AuthorityState, object_ref: &ObjectRef) {
    let response = authority
        .handle_object_info_request(ObjectInfoRequest {
            object_id: object_ref.0,
            request_kind: ObjectInfoRequestKind::LatestObjectInfo(None),
        })
        .await
        .unwrap();
    assert_eq!(&response.requested_object_reference.unwrap(), object_ref,);
}

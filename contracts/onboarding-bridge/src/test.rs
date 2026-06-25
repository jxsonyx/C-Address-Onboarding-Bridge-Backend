#![cfg(test)]

use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::Address as _,
    Address, Env, IntoVal, MuxedAddress, String, Symbol, Vec,
};

use super::*;

// ---------------------------------------------------------------------------
// Test token — minimal SEP-41 compliant token.
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone)]
enum TK {
    Bal(Address),
}

#[contract]
struct TestToken;

#[contractimpl]
impl TestToken {
    pub fn transfer(env: Env, from: Address, to: MuxedAddress, amount: i128) {
        from.require_auth();
        let to_addr = to.address();
        let from_bal = env
            .storage()
            .persistent()
            .get::<TK, i128>(&TK::Bal(from.clone()))
            .unwrap_or(0);
        let to_bal = env
            .storage()
            .persistent()
            .get::<TK, i128>(&TK::Bal(to_addr.clone()))
            .unwrap_or(0);
        env.storage()
            .persistent()
            .set(&TK::Bal(from), &(from_bal - amount));
        env.storage()
            .persistent()
            .set(&TK::Bal(to_addr), &(to_bal + amount));
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        env.storage()
            .persistent()
            .get::<TK, i128>(&TK::Bal(id))
            .unwrap_or(0)
    }

    pub fn mint(env: Env, to: Address, amount: i128) {
        let bal = env
            .storage()
            .persistent()
            .get::<TK, i128>(&TK::Bal(to.clone()))
            .unwrap_or(0);
        env.storage()
            .persistent()
            .set(&TK::Bal(to), &(bal + amount));
    }

    pub fn decimals(_env: Env) -> u32 { 7 }
    pub fn name(env: Env) -> String { String::from_str(&env, "TestToken") }
    pub fn symbol(env: Env) -> String { String::from_str(&env, "TEST") }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn setup() -> (Env, OnboardingBridgeClient<'static>) {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let id = env.register_contract(None, OnboardingBridge);
    let client = OnboardingBridgeClient::new(&env, &id);
    let env_cloned = env.clone();
    drop(env);
    (env_cloned, client)
}

fn initialized() -> (Env, OnboardingBridgeClient<'static>, Address) {
    let (env, bridge) = setup();
    let admin = Address::generate(&env);
    bridge.initialize(&admin, &30).unwrap();
    (env, bridge, admin)
}

// ---------------------------------------------------------------------------
// State & admin tests
// ---------------------------------------------------------------------------

#[test]
fn test_initialize() {
    let (env, bridge) = setup();
    let admin = Address::generate(&env);
    bridge.initialize(&admin, &30).unwrap();
    assert_eq!(bridge.admin().unwrap(), admin);
    assert_eq!(bridge.fee_bps(), 30);
    assert_eq!(bridge.version(), 1);
    assert_eq!(bridge.schema_version(), SCHEMA_VERSION);
}

#[test]
fn test_double_initialize_returns_error() {
    let (env, bridge) = setup();
    let admin = Address::generate(&env);
    bridge.initialize(&admin, &30).unwrap();
    let err = bridge.try_initialize(&admin, &50).unwrap_err().unwrap();
    assert_eq!(err, ContractError::AlreadyInitialized);
}

#[test]
fn test_invalid_fee_bps_returns_error() {
    let (env, bridge) = setup();
    let admin = Address::generate(&env);
    let err = bridge.try_initialize(&admin, &10001).unwrap_err().unwrap();
    assert_eq!(err, ContractError::InvalidFeeBps);
}

#[test]
fn test_set_fee() {
    let (env, bridge, _admin) = initialized();
    let _ = &env;
    bridge.set_fee(&50).unwrap();
    assert_eq!(bridge.fee_bps(), 50);
}

#[test]
fn test_set_fee_at_boundary() {
    let (env, bridge, _admin) = initialized();
    let _ = &env;
    bridge.set_fee(&0).unwrap();
    assert_eq!(bridge.fee_bps(), 0);
    bridge.set_fee(&10000).unwrap();
    assert_eq!(bridge.fee_bps(), 10000);
}

#[test]
fn test_set_fee_invalid() {
    let (env, bridge, _admin) = initialized();
    let _ = &env;
    let err = bridge.try_set_fee(&10001).unwrap_err().unwrap();
    assert_eq!(err, ContractError::InvalidFeeBps);
}

#[test]
fn test_initial_state() {
    let (env, bridge, _admin) = initialized();
    let _ = &env;
    assert_eq!(bridge.accumulated_fees(), 0);
    assert_eq!(bridge.version(), 1);
}

// ---------------------------------------------------------------------------
// Zero-amount guard
// ---------------------------------------------------------------------------

#[test]
fn test_fund_zero_amount_rejected() {
    let (env, bridge, _admin) = initialized();
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let token = Address::generate(&env);
    let memo = String::from_str(&env, "zero");

    let err = bridge
        .try_fund_c_address(&source, &target, &token, &0, &memo)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::ZeroAmount);
}

#[test]
fn test_fund_negative_amount_rejected() {
    let (env, bridge, _admin) = initialized();
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let token = Address::generate(&env);
    let memo = String::from_str(&env, "neg");

    let err = bridge
        .try_fund_c_address(&source, &target, &token, &-1, &memo)
        .unwrap_err()
        .unwrap();
    assert_eq!(err, ContractError::ZeroAmount);
}

// ---------------------------------------------------------------------------
// Fund & withdraw logic tests
// ---------------------------------------------------------------------------

#[test]
fn test_fund_c_address_tracks_fees() {
    let (env, bridge, _admin) = initialized();
    bridge.set_fee(&100).unwrap();
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let token = Address::generate(&env);
    let memo = String::from_str(&env, "fund test");

    let fee = bridge.fund_c_address(&source, &target, &token, &1000, &memo).unwrap();
    assert_eq!(fee, 10);
    assert_eq!(bridge.accumulated_fees(), 10);
}

#[test]
fn test_fund_with_zero_fee() {
    let (env, bridge) = setup();
    let admin = Address::generate(&env);
    bridge.initialize(&admin, &0).unwrap();
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let token = Address::generate(&env);
    let memo = String::from_str(&env, "no fee");

    let fee = bridge.fund_c_address(&source, &target, &token, &500, &memo).unwrap();
    assert_eq!(fee, 0);
    assert_eq!(bridge.accumulated_fees(), 0);
}

#[test]
fn test_withdraw_fees() {
    let (env, bridge, admin) = initialized();
    bridge.set_fee(&200).unwrap();
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let token = Address::generate(&env);

    bridge.fund_c_address(&source, &target, &token, &1000, &String::from_str(&env, "t")).unwrap();
    assert_eq!(bridge.accumulated_fees(), 20);

    let withdrawn = bridge.withdraw_fees(&admin, &token, &0).unwrap();
    assert_eq!(withdrawn, 20);
    assert_eq!(bridge.accumulated_fees(), 0);
}

#[test]
fn test_withdraw_fees_partial() {
    let (env, bridge, admin) = initialized();
    bridge.set_fee(&100).unwrap();
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let token = Address::generate(&env);

    bridge.fund_c_address(&source, &target, &token, &1000, &String::from_str(&env, "t")).unwrap();
    let withdrawn = bridge.withdraw_fees(&admin, &token, &4).unwrap();
    assert_eq!(withdrawn, 4);
    assert_eq!(bridge.accumulated_fees(), 6);
}

#[test]
fn test_withdraw_fees_excessive_returns_error() {
    let (env, bridge, admin) = initialized();
    bridge.set_fee(&100).unwrap();
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let token = Address::generate(&env);

    bridge.fund_c_address(&source, &target, &token, &1000, &String::from_str(&env, "t")).unwrap();

    let err = bridge.try_withdraw_fees(&admin, &token, &999).unwrap_err().unwrap();
    assert_eq!(err, ContractError::InsufficientFees);
}

#[test]
fn test_route_from_exchange() {
    let (env, bridge, _admin) = initialized();
    bridge.set_fee(&50).unwrap();
    let exchange = Address::generate(&env);
    let target = Address::generate(&env);
    let token = Address::generate(&env);
    let memo = String::from_str(&env, "cex test");

    let fee = bridge.route_from_exchange(&exchange, &target, &token, &500, &memo).unwrap();
    assert_eq!(fee, 2);
    assert_eq!(bridge.accumulated_fees(), 2);
}

#[test]
fn test_multiple_fund_accumulates_fees() {
    let (env, bridge, _admin) = initialized();
    bridge.set_fee(&100).unwrap();
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let token = Address::generate(&env);

    bridge.fund_c_address(&source, &target, &token, &1000, &String::from_str(&env, "t1")).unwrap();
    assert_eq!(bridge.accumulated_fees(), 10);
    bridge.fund_c_address(&source, &target, &token, &2000, &String::from_str(&env, "t2")).unwrap();
    assert_eq!(bridge.accumulated_fees(), 30);
    bridge.fund_c_address(&source, &target, &token, &3000, &String::from_str(&env, "t3")).unwrap();
    assert_eq!(bridge.accumulated_fees(), 60);
}

// ---------------------------------------------------------------------------
// Schema version / migration
// ---------------------------------------------------------------------------

#[test]
fn test_schema_version_set_on_init() {
    let (env, bridge) = setup();
    let admin = Address::generate(&env);
    bridge.initialize(&admin, &30).unwrap();
    assert_eq!(bridge.schema_version(), SCHEMA_VERSION);
}

#[test]
fn test_migrate_is_idempotent() {
    let (env, bridge, _admin) = initialized();
    let _ = &env;
    // migrate on an already-current schema is a no-op, should not error
    bridge.migrate().unwrap();
    assert_eq!(bridge.schema_version(), SCHEMA_VERSION);
}

// ---------------------------------------------------------------------------
// Direct token transfer (verifies cross-contract invoke works from test env)
// ---------------------------------------------------------------------------

#[test]
fn test_token_transfer_direct() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let alice = Address::generate(&env);
    let bob = Address::generate(&env);
    let token_id = env.register_contract(None, TestToken);

    let _: () = env.invoke_contract(
        &token_id,
        &Symbol::new(&env, "mint"),
        Vec::from_array(&env, [alice.clone().into_val(&env), 500i128.into_val(&env)]),
    );
    let _: () = env.invoke_contract(
        &token_id,
        &Symbol::new(&env, "transfer"),
        Vec::from_array(
            &env,
            [
                alice.clone().into_val(&env),
                MuxedAddress::from(&bob).into_val(&env),
                200i128.into_val(&env),
            ],
        ),
    );

    let alice_bal: i128 = env.invoke_contract(
        &token_id,
        &Symbol::new(&env, "balance"),
        Vec::from_array(&env, [alice.clone().into_val(&env)]),
    );
    let bob_bal: i128 = env.invoke_contract(
        &token_id,
        &Symbol::new(&env, "balance"),
        Vec::from_array(&env, [bob.clone().into_val(&env)]),
    );

    assert_eq!(alice_bal, 300);
    assert_eq!(bob_bal, 200);
}

// ---------------------------------------------------------------------------
// Full integration scenario
// ---------------------------------------------------------------------------

#[test]
fn test_full_scenario() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let admin = Address::generate(&env);
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let token_addr = Address::generate(&env);

    let bridge_id = env.register_contract(None, OnboardingBridge);
    let bridge = OnboardingBridgeClient::new(&env, &bridge_id);
    bridge.initialize(&admin, &100).unwrap();

    let fee = bridge.fund_c_address(
        &source, &target, &token_addr, &1000, &String::from_str(&env, "full test"),
    ).unwrap();
    assert_eq!(fee, 10);
    assert_eq!(bridge.accumulated_fees(), 10);

    let withdrawn = bridge.withdraw_fees(&admin, &token_addr, &0).unwrap();
    assert_eq!(withdrawn, 10);
    assert_eq!(bridge.accumulated_fees(), 0);
}

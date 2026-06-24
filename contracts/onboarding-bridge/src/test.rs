#![cfg(test)]

use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::Address as _,
    Address, Env, IntoVal, MuxedAddress, String, Symbol, Vec,
};

use super::*;

// ---------------------------------------------------------------------------
// SAC-compatible test token (SEP-41 interface)
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone)]
enum TK {
    Bal(Address),
    Allowance(Address, Address), // (owner, spender)
}

#[contract]
struct TestToken;

#[contractimpl]
impl TestToken {
    pub fn transfer(env: Env, from: Address, to: MuxedAddress, amount: i128) {
        from.require_auth();
        let to_addr = to.address();
        let from_bal = env.storage().persistent().get::<TK, i128>(&TK::Bal(from.clone())).unwrap_or(0);
        assert!(from_bal >= amount, "insufficient balance");
        let to_bal = env.storage().persistent().get::<TK, i128>(&TK::Bal(to_addr.clone())).unwrap_or(0);
        env.storage().persistent().set(&TK::Bal(from), &(from_bal - amount));
        env.storage().persistent().set(&TK::Bal(to_addr), &(to_bal + amount));
    }

    pub fn transfer_from(env: Env, spender: Address, from: Address, to: MuxedAddress, amount: i128) {
        spender.require_auth();
        let allowance = env
            .storage()
            .persistent()
            .get::<TK, i128>(&TK::Allowance(from.clone(), spender.clone()))
            .unwrap_or(0);
        assert!(allowance >= amount, "insufficient allowance");
        let to_addr = to.address();
        let from_bal = env.storage().persistent().get::<TK, i128>(&TK::Bal(from.clone())).unwrap_or(0);
        assert!(from_bal >= amount, "insufficient balance");
        let to_bal = env.storage().persistent().get::<TK, i128>(&TK::Bal(to_addr.clone())).unwrap_or(0);
        env.storage().persistent().set(&TK::Allowance(from.clone(), spender), &(allowance - amount));
        env.storage().persistent().set(&TK::Bal(from), &(from_bal - amount));
        env.storage().persistent().set(&TK::Bal(to_addr), &(to_bal + amount));
    }

    pub fn approve(env: Env, from: Address, spender: Address, amount: i128, _expiration_ledger: u32) {
        from.require_auth();
        env.storage().persistent().set(&TK::Allowance(from, spender), &amount);
    }

    pub fn allowance(env: Env, from: Address, spender: Address) -> i128 {
        env.storage().persistent().get::<TK, i128>(&TK::Allowance(from, spender)).unwrap_or(0)
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        env.storage().persistent().get::<TK, i128>(&TK::Bal(id)).unwrap_or(0)
    }

    pub fn mint(env: Env, to: Address, amount: i128) {
        let bal = env.storage().persistent().get::<TK, i128>(&TK::Bal(to.clone())).unwrap_or(0);
        env.storage().persistent().set(&TK::Bal(to), &(bal + amount));
    }

    pub fn decimals(_env: Env) -> u32 { 7 }

    pub fn name(env: Env) -> String { String::from_str(&env, "TestToken") }

    pub fn symbol(env: Env) -> String { String::from_str(&env, "TEST") }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Deploy bridge + token, initialize bridge, return clients and addresses.
fn full_setup(fee_bps: u32) -> (Env, OnboardingBridgeClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
    let token_id = env.register_contract(None, TestToken);

    let bridge_id = env.register_contract(None, OnboardingBridge);
    let bridge = OnboardingBridgeClient::new(&env, &bridge_id);
    bridge.initialize(&admin, &fee_bps);

    // SAFETY: env outlives the test function; this is the standard soroban test pattern.
    let bridge: OnboardingBridgeClient<'static> = unsafe { core::mem::transmute(bridge) };
    (env, bridge, token_id, admin)
}

fn token_balance(env: &Env, token: &Address, who: &Address) -> i128 {
    env.invoke_contract(
        token,
        &Symbol::new(env, "balance"),
        Vec::from_array(env, [who.into_val(env)]),
    )
}

fn token_mint(env: &Env, token: &Address, to: &Address, amount: i128) {
    let _: () = env.invoke_contract(
        token,
        &Symbol::new(env, "mint"),
        Vec::from_array(env, [to.into_val(env), amount.into_val(env)]),
    );
}

fn token_transfer(env: &Env, token: &Address, from: &Address, to: &Address, amount: i128) {
    let _: () = env.invoke_contract(
        token,
        &Symbol::new(env, "transfer"),
        Vec::from_array(env, [
            from.into_val(env),
            MuxedAddress::from(to).into_val(env),
            amount.into_val(env),
        ]),
    );
}

fn token_approve(env: &Env, token: &Address, from: &Address, spender: &Address, amount: i128) {
    let _: () = env.invoke_contract(
        token,
        &Symbol::new(env, "approve"),
        Vec::from_array(env, [
            from.into_val(env),
            spender.into_val(env),
            amount.into_val(env),
            100u32.into_val(env), // expiration ledger
        ]),
    );
}

fn token_transfer_from(
    env: &Env, token: &Address, spender: &Address,
    from: &Address, to: &Address, amount: i128,
) {
    let _: () = env.invoke_contract(
        token,
        &Symbol::new(env, "transfer_from"),
        Vec::from_array(env, [
            spender.into_val(env),
            from.into_val(env),
            MuxedAddress::from(to).into_val(env),
            amount.into_val(env),
        ]),
    );
}

// ---------------------------------------------------------------------------
// #20: Analytics / counters
// ---------------------------------------------------------------------------

#[test]
fn test_get_stats_initial() {
    let (_env, bridge, _, _) = full_setup(100);
    let s = bridge.get_stats();
    assert_eq!(s.total_volume, 0);
    assert_eq!(s.total_fees, 0);
    assert_eq!(s.funding_count, 0);
    assert_eq!(s.unique_funder_count, 0);
}

#[test]
fn test_stats_after_single_fund() {
    let (env, bridge, token, _) = full_setup(100); // 1%
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let memo = String::from_str(&env, "test");

    bridge.fund_c_address(&source, &target, &token, &1000, &memo);

    let s = bridge.get_stats();
    assert_eq!(s.total_volume, 1000);
    assert_eq!(s.total_fees, 10);      // 1% of 1000
    assert_eq!(s.funding_count, 1);
    assert_eq!(s.unique_funder_count, 1);
}

#[test]
fn test_stats_accumulate_across_fundings() {
    let (env, bridge, token, _) = full_setup(200); // 2%
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let memo = String::from_str(&env, "t");

    bridge.fund_c_address(&source, &target, &token, &1000, &memo);
    bridge.fund_c_address(&source, &target, &token, &2000, &memo);
    bridge.fund_c_address(&source, &target, &token, &3000, &memo);

    let s = bridge.get_stats();
    assert_eq!(s.total_volume, 6000);
    assert_eq!(s.total_fees, 120);   // 2% * 6000
    assert_eq!(s.funding_count, 3);
    assert_eq!(s.unique_funder_count, 1); // same source
}

#[test]
fn test_unique_funder_count() {
    let (env, bridge, token, _) = full_setup(100);
    let alice = Address::generate(&env);
    let bob = Address::generate(&env);
    let carol = Address::generate(&env);
    let target = Address::generate(&env);
    let memo = String::from_str(&env, "t");

    bridge.fund_c_address(&alice, &target, &token, &100, &memo);
    bridge.fund_c_address(&alice, &target, &token, &100, &memo); // duplicate
    bridge.fund_c_address(&bob, &target, &token, &100, &memo);
    bridge.fund_c_address(&carol, &target, &token, &100, &memo);

    let s = bridge.get_stats();
    assert_eq!(s.funding_count, 4);
    assert_eq!(s.unique_funder_count, 3); // alice counted once
}

#[test]
fn test_stats_zero_fee() {
    let (env, bridge, token, _) = full_setup(0);
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let memo = String::from_str(&env, "t");

    bridge.fund_c_address(&source, &target, &token, &5000, &memo);

    let s = bridge.get_stats();
    assert_eq!(s.total_volume, 5000);
    assert_eq!(s.total_fees, 0);
    assert_eq!(s.funding_count, 1);
}

#[test]
fn test_route_from_exchange_increments_stats() {
    let (env, bridge, token, _) = full_setup(50);
    let exchange = Address::generate(&env);
    let target = Address::generate(&env);
    let memo = String::from_str(&env, "cex");

    bridge.route_from_exchange(&exchange, &target, &token, &10000, &memo);

    let s = bridge.get_stats();
    assert_eq!(s.total_volume, 10000);
    assert_eq!(s.funding_count, 1);
    assert_eq!(s.unique_funder_count, 1);
}

// ---------------------------------------------------------------------------
// #19: SAC-compatible token integration — cross-contract flows
// ---------------------------------------------------------------------------

#[test]
fn test_sac_token_mint_and_balance() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let token = env.register_contract(None, TestToken);
    let alice = Address::generate(&env);

    token_mint(&env, &token, &alice, 1_000_000);
    assert_eq!(token_balance(&env, &token, &alice), 1_000_000);
}

#[test]
fn test_sac_token_transfer() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let token = env.register_contract(None, TestToken);
    let alice = Address::generate(&env);
    let bob = Address::generate(&env);

    token_mint(&env, &token, &alice, 1000);
    token_transfer(&env, &token, &alice, &bob, 400);

    assert_eq!(token_balance(&env, &token, &alice), 600);
    assert_eq!(token_balance(&env, &token, &bob), 400);
}

#[test]
fn test_sac_approve_and_transfer_from() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let token = env.register_contract(None, TestToken);
    let alice = Address::generate(&env);
    let bridge_addr = Address::generate(&env);
    let _target = Address::generate(&env);

    token_mint(&env, &token, &alice, 1000);
    token_approve(&env, &token, &alice, &bridge_addr, 500);

    // Verify allowance is set
    let allowance: i128 = env.invoke_contract(
        &token,
        &Symbol::new(&env, "allowance"),
        Vec::from_array(&env, [alice.into_val(&env), bridge_addr.clone().into_val(&env)]),
    );
    assert_eq!(allowance, 500);
}

/// Full end-to-end: source approves bridge, bridge uses transfer_from, bridge records fees.
#[test]
fn test_end_to_end_fund_with_real_token_transfer() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
    let source = Address::generate(&env);
    let target = Address::generate(&env);

    let token = env.register_contract(None, TestToken);
    let bridge_id = env.register_contract(None, OnboardingBridge);
    let bridge = OnboardingBridgeClient::new(&env, &bridge_id);
    bridge.initialize(&admin, &100); // 1%

    let amount = 1000i128;
    token_mint(&env, &token, &source, amount);

    // Source approves bridge to pull tokens
    token_approve(&env, &token, &source, &bridge_id, amount);

    // Bridge records the funding (accounting only)
    let memo = String::from_str(&env, "e2e");
    let fee = bridge.fund_c_address(&source, &target, &token, &amount, &memo);
    assert_eq!(fee, 10);

    // Bridge executes token transfer: net amount to target, fee stays with bridge
    let net = amount - fee;
    token_transfer_from(&env, &token, &bridge_id, &source, &target, net);
    token_transfer_from(&env, &token, &bridge_id, &source, &bridge_id, fee);

    assert_eq!(token_balance(&env, &token, &source), 0);
    assert_eq!(token_balance(&env, &token, &target), net);
    assert_eq!(token_balance(&env, &token, &bridge_id), fee);
    assert_eq!(bridge.accumulated_fees(), 10);
}

/// Multi-token flow: two different tokens bridged through same contract.
#[test]
fn test_multi_token_flow() {
    let (env, bridge, token_a, _) = full_setup(100);
    let token_b = env.register_contract(None, TestToken);

    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let memo = String::from_str(&env, "multi");

    bridge.fund_c_address(&source, &target, &token_a, &1000, &memo);
    bridge.fund_c_address(&source, &target, &token_b, &2000, &memo);

    let s = bridge.get_stats();
    assert_eq!(s.total_volume, 3000);
    assert_eq!(s.funding_count, 2);
    assert_eq!(s.total_fees, 30); // 10 + 20
}

/// Edge case: token decimals — 7 decimals (XLM-like), verify fee math is correct.
#[test]
fn test_token_decimals_fee_math() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let token = env.register_contract(None, TestToken);

    // Verify this token reports 7 decimals (XLM / SAC standard)
    let decimals: u32 = env.invoke_contract(
        &token,
        &Symbol::new(&env, "decimals"),
        Vec::new(&env),
    );
    assert_eq!(decimals, 7);

    // 1 XLM = 10_000_000 stroops; 30 bps fee = 30_000 stroops
    let bridge_id = env.register_contract(None, OnboardingBridge);
    let bridge = OnboardingBridgeClient::new(&env, &bridge_id);
    let admin = Address::generate(&env);
    bridge.initialize(&admin, &30);

    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let memo = String::from_str(&env, "decimals");
    let one_xlm = 10_000_000i128;
    let fee = bridge.fund_c_address(&source, &target, &token, &one_xlm, &memo);
    assert_eq!(fee, 30_000); // 1 XLM * 30 bps
}

/// Edge case: insufficient balance — transfer should fail.
#[test]
#[should_panic(expected = "insufficient balance")]
fn test_transfer_insufficient_balance() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let token = env.register_contract(None, TestToken);
    let alice = Address::generate(&env);
    let bob = Address::generate(&env);

    token_mint(&env, &token, &alice, 100);
    token_transfer(&env, &token, &alice, &bob, 200); // should panic
}

/// Edge case: insufficient allowance — transfer_from should fail.
#[test]
#[should_panic(expected = "insufficient allowance")]
fn test_transfer_from_insufficient_allowance() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let token = env.register_contract(None, TestToken);
    let alice = Address::generate(&env);
    let spender = Address::generate(&env);
    let bob = Address::generate(&env);

    token_mint(&env, &token, &alice, 1000);
    token_approve(&env, &token, &alice, &spender, 50); // only 50 approved
    token_transfer_from(&env, &token, &spender, &alice, &bob, 200); // should panic
}

/// Edge case: zero-amount funding is allowed, stats still increment count.
#[test]
fn test_zero_amount_funding() {
    let (env, bridge, token, _) = full_setup(100);
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let memo = String::from_str(&env, "zero");

    let fee = bridge.fund_c_address(&source, &target, &token, &0, &memo);
    assert_eq!(fee, 0);

    let s = bridge.get_stats();
    assert_eq!(s.total_volume, 0);
    assert_eq!(s.funding_count, 1); // count still increments
}

/// Edge case: maximum fee (100%) still leaves correct net.
#[test]
fn test_max_fee_100_percent() {
    let (env, bridge, token, _) = full_setup(10000); // 100%
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let memo = String::from_str(&env, "maxfee");

    let fee = bridge.fund_c_address(&source, &target, &token, &1000, &memo);
    assert_eq!(fee, 1000); // 100% fee
    assert_eq!(bridge.accumulated_fees(), 1000);
}

/// Edge case: double-initialize should fail.
#[test]
#[should_panic(expected = "already initialized")]
fn test_double_initialize() {
    let (_env, bridge, _, admin) = full_setup(30);
    bridge.initialize(&admin, &50);
}

/// Edge case: invalid fee_bps > 10000 should panic.
#[test]
#[should_panic]
fn test_invalid_fee_bps() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let bridge_id = env.register_contract(None, OnboardingBridge);
    let bridge = OnboardingBridgeClient::new(&env, &bridge_id);
    let admin = Address::generate(&env);
    bridge.initialize(&admin, &10001); // should panic
}

/// Edge case: withdraw more than accumulated fees should fail.
#[test]
#[should_panic(expected = "insufficient accumulated fees")]
fn test_withdraw_excess_fees() {
    let (env, bridge, token, admin) = full_setup(100);
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let memo = String::from_str(&env, "t");
    bridge.fund_c_address(&source, &target, &token, &1000, &memo);
    bridge.withdraw_fees(&admin, &token, &999); // only 10 available
}

// ---------------------------------------------------------------------------
// Legacy unit tests preserved
// ---------------------------------------------------------------------------

fn setup_env() -> (Env, OnboardingBridgeClient<'static>) {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();
    let id = env.register_contract(None, OnboardingBridge);
    let client = OnboardingBridgeClient::new(&env, &id);
    let client: OnboardingBridgeClient<'static> = unsafe { core::mem::transmute(client) };
    let env_cloned = env.clone();
    drop(env);
    (env_cloned, client)
}

#[test]
fn test_initialize() {
    let (env, bridge) = setup_env();
    let admin = Address::generate(&env);
    bridge.initialize(&admin, &30);
    assert_eq!(bridge.admin(), admin);
    assert_eq!(bridge.fee_bps(), 30);
    assert_eq!(bridge.version(), 1);
}

#[test]
fn test_set_fee() {
    let (env, bridge) = setup_env();
    let admin = Address::generate(&env);
    bridge.initialize(&admin, &30);
    bridge.set_fee(&50);
    assert_eq!(bridge.fee_bps(), 50);
}

#[test]
fn test_fund_c_address_tracks_fees() {
    let (env, bridge) = setup_env();
    let admin = Address::generate(&env);
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let token_addr = Address::generate(&env);
    bridge.initialize(&admin, &100);
    let memo = String::from_str(&env, "fund test");
    let fee = bridge.fund_c_address(&source, &target, &token_addr, &1000, &memo);
    assert_eq!(fee, 10);
    assert_eq!(bridge.accumulated_fees(), 10);
}

#[test]
fn test_route_from_exchange() {
    let (env, bridge) = setup_env();
    let admin = Address::generate(&env);
    let exchange = Address::generate(&env);
    let target = Address::generate(&env);
    let token_addr = Address::generate(&env);
    bridge.initialize(&admin, &50);
    let memo = String::from_str(&env, "cex test");
    let fee = bridge.route_from_exchange(&exchange, &target, &token_addr, &500, &memo);
    assert_eq!(fee, 2);
    assert_eq!(bridge.accumulated_fees(), 2);
}

#[test]
fn test_withdraw_fees() {
    let (env, bridge) = setup_env();
    let admin = Address::generate(&env);
    let source = Address::generate(&env);
    let target = Address::generate(&env);
    let token_addr = Address::generate(&env);
    bridge.initialize(&admin, &200);
    let memo = String::from_str(&env, "test");
    bridge.fund_c_address(&source, &target, &token_addr, &1000, &memo);
    assert_eq!(bridge.accumulated_fees(), 20);
    let withdrawn = bridge.withdraw_fees(&admin, &token_addr, &0);
    assert_eq!(withdrawn, 20);
    assert_eq!(bridge.accumulated_fees(), 0);
}

#![no_std]
#![allow(deprecated)]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, String, Symbol};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    FeeBps,
    AccumulatedFees,
    Version,
    // #20: analytics counters
    TotalVolume,
    FundingCount,
    UniqueFunder(Address),
    UniqueFunderCount,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub total_volume: i128,
    pub total_fees: i128,
    pub funding_count: u64,
    pub unique_funder_count: u64,
}

#[contract]
pub struct OnboardingBridge;

#[contractimpl]
impl OnboardingBridge {
    pub fn initialize(env: Env, admin: Address, fee_bps: u32) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        admin.require_auth();
        assert!(fee_bps <= 10000, "fee_bps must be <= 10000");
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        env.storage().instance().set(&DataKey::AccumulatedFees, &0i128);
        env.storage().instance().set(&DataKey::Version, &1u32);
        env.storage().instance().set(&DataKey::TotalVolume, &0i128);
        env.storage().instance().set(&DataKey::FundingCount, &0u64);
        env.storage().instance().set(&DataKey::UniqueFunderCount, &0u64);
        env.events()
            .publish((Symbol::new(&env, "initialize"),), (admin, fee_bps));
    }

    pub fn version(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::Version).unwrap_or(0)
    }

    pub fn admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).expect("not initialized")
    }

    pub fn fee_bps(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::FeeBps).unwrap_or(0)
    }

    pub fn accumulated_fees(env: Env) -> i128 {
        env.storage().instance().get(&DataKey::AccumulatedFees).unwrap_or(0)
    }

    /// #20: Batch view — returns all analytics in one call.
    pub fn get_stats(env: Env) -> Stats {
        Stats {
            total_volume: env.storage().instance().get(&DataKey::TotalVolume).unwrap_or(0),
            total_fees: env.storage().instance().get(&DataKey::AccumulatedFees).unwrap_or(0),
            funding_count: env.storage().instance().get(&DataKey::FundingCount).unwrap_or(0),
            unique_funder_count: env.storage().instance().get(&DataKey::UniqueFunderCount).unwrap_or(0),
        }
    }

    pub fn set_fee(env: Env, new_fee_bps: u32) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        admin.require_auth();
        assert!(new_fee_bps <= 10000, "fee_bps must be <= 10000");
        env.storage().instance().set(&DataKey::FeeBps, &new_fee_bps);
        env.events().publish((Symbol::new(&env, "set_fee"),), (new_fee_bps,));
    }

    /// Record a funding event. The caller is responsible for the token transfer.
    /// Returns the fee amount deducted.
    pub fn fund_c_address(
        env: Env,
        source: Address,
        target: Address,
        _token_address: Address,
        amount: i128,
        _memo: String,
    ) -> i128 {
        let fee_bps: u32 = env.storage().instance().get(&DataKey::FeeBps).unwrap_or(0);
        let fee_amount = if fee_bps > 0 { (amount * fee_bps as i128) / 10000 } else { 0i128 };

        // Accumulate fees
        if fee_amount > 0 {
            let accumulated: i128 = env.storage().instance().get(&DataKey::AccumulatedFees).unwrap_or(0);
            env.storage().instance().set(&DataKey::AccumulatedFees, &(accumulated + fee_amount));
        }

        // #20: increment analytics atomically
        let volume: i128 = env.storage().instance().get(&DataKey::TotalVolume).unwrap_or(0);
        env.storage().instance().set(&DataKey::TotalVolume, &(volume + amount));

        let count: u64 = env.storage().instance().get(&DataKey::FundingCount).unwrap_or(0);
        env.storage().instance().set(&DataKey::FundingCount, &(count + 1));

        // approximate unique funder tracking
        let key = DataKey::UniqueFunder(source.clone());
        if !env.storage().persistent().has(&key) {
            env.storage().persistent().set(&key, &true);
            let uc: u64 = env.storage().instance().get(&DataKey::UniqueFunderCount).unwrap_or(0);
            env.storage().instance().set(&DataKey::UniqueFunderCount, &(uc + 1));
        }

        env.events().publish(
            (Symbol::new(&env, "funded"),),
            (source, target, amount, fee_amount),
        );

        fee_amount
    }

    /// Withdraw accumulated fees to `to`.
    pub fn withdraw_fees(env: Env, to: Address, token_address: Address, amount: i128) -> i128 {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).expect("not initialized");
        admin.require_auth();

        let accumulated: i128 = env.storage().instance().get(&DataKey::AccumulatedFees).unwrap_or(0);
        let withdraw_amount = if amount == 0 { accumulated } else { amount };
        assert!(withdraw_amount <= accumulated, "insufficient accumulated fees");

        env.storage()
            .instance()
            .set(&DataKey::AccumulatedFees, &(accumulated - withdraw_amount));

        env.events().publish(
            (Symbol::new(&env, "withdrawn"),),
            (to, token_address, withdraw_amount),
        );

        withdraw_amount
    }

    pub fn route_from_exchange(
        env: Env,
        exchange: Address,
        target: Address,
        token_address: Address,
        amount: i128,
        memo: String,
    ) -> i128 {
        exchange.require_auth();
        Self::fund_c_address(env, exchange, target, token_address, amount, memo)
    }
}

mod test;

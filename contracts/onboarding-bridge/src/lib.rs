#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, String, Symbol};

/// Current on-chain storage schema version. Bump this whenever a storage key
/// is added, renamed, or its value type changes.
pub const SCHEMA_VERSION: u32 = 1;

/// Structured error type returned by all public functions.
#[contracterror]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContractError {
    /// Contract has already been initialized.
    AlreadyInitialized = 1,
    /// Contract has not been initialized yet.
    NotInitialized = 2,
    /// Caller is not the contract admin.
    Unauthorized = 3,
    /// fee_bps exceeds 10000 (100 %).
    InvalidFeeBps = 4,
    /// Transfer amount must be > 0.
    ZeroAmount = 5,
    /// Withdrawal would exceed accumulated fees.
    InsufficientFees = 6,
    /// On-chain schema version is newer than this code understands.
    IncompatibleSchema = 7,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    FeeBps,
    AccumulatedFees,
    /// Logical contract version (user-visible, incremented on each upgrade).
    Version,
    /// Storage schema version (incremented when keys/types change).
    SchemaVersion,
}

#[contract]
pub struct OnboardingBridge;

#[contractimpl]
impl OnboardingBridge {
    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    pub fn initialize(env: Env, admin: Address, fee_bps: u32) -> Result<(), ContractError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ContractError::AlreadyInitialized);
        }
        admin.require_auth();
        if fee_bps > 10000 {
            return Err(ContractError::InvalidFeeBps);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        env.storage().instance().set(&DataKey::AccumulatedFees, &0i128);
        env.storage().instance().set(&DataKey::Version, &1u32);
        env.storage().instance().set(&DataKey::SchemaVersion, &SCHEMA_VERSION);
        env.events().publish((Symbol::new(&env, "initialize"),), (admin, fee_bps));
        Ok(())
    }

    /// Replace the contract WASM with `new_wasm_hash`.
    ///
    /// Only the admin may call this. Storage persists across upgrades.
    /// After upgrading, call `migrate()` if the new code bumps SCHEMA_VERSION.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), ContractError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ContractError::NotInitialized)?;
        admin.require_auth();
        env.deployer().update_current_contract_wasm(new_wasm_hash.clone());
        env.events().publish(
            (Symbol::new(&env, "upgrade"),),
            (admin, new_wasm_hash),
        );
        Ok(())
    }

    /// Run after an upgrade when SCHEMA_VERSION has been bumped.
    ///
    /// Validates that the on-chain schema version is compatible and writes the
    /// new schema version. Add field-migration logic here when needed.
    pub fn migrate(env: Env) -> Result<(), ContractError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ContractError::NotInitialized)?;
        admin.require_auth();

        let on_chain: u32 = env
            .storage()
            .instance()
            .get(&DataKey::SchemaVersion)
            .unwrap_or(1);

        if on_chain > SCHEMA_VERSION {
            return Err(ContractError::IncompatibleSchema);
        }

        // Place per-version migration steps here, e.g.:
        //   if on_chain < 2 { /* rename key X to Y */ }

        env.storage().instance().set(&DataKey::SchemaVersion, &SCHEMA_VERSION);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Getters
    // -----------------------------------------------------------------------

    pub fn version(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::Version).unwrap_or(0)
    }

    pub fn schema_version(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::SchemaVersion).unwrap_or(0)
    }

    pub fn admin(env: Env) -> Result<Address, ContractError> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ContractError::NotInitialized)
    }

    pub fn fee_bps(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::FeeBps).unwrap_or(0)
    }

    pub fn accumulated_fees(env: Env) -> i128 {
        env.storage().instance().get(&DataKey::AccumulatedFees).unwrap_or(0)
    }

    // -----------------------------------------------------------------------
    // Admin operations
    // -----------------------------------------------------------------------

    pub fn set_fee(env: Env, new_fee_bps: u32) -> Result<(), ContractError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ContractError::NotInitialized)?;
        admin.require_auth();
        if new_fee_bps > 10000 {
            return Err(ContractError::InvalidFeeBps);
        }
        env.storage().instance().set(&DataKey::FeeBps, &new_fee_bps);
        env.events().publish((Symbol::new(&env, "set_fee"),), (new_fee_bps,));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Core funding
    // -----------------------------------------------------------------------

    /// Record a funding event. The caller is responsible for the token transfer.
    /// Returns the fee amount deducted.
    ///
    /// Errors:
    /// - `ZeroAmount` — amount is 0
    /// - `NotInitialized` — contract not initialized
    pub fn fund_c_address(
        env: Env,
        source: Address,
        target: Address,
        _token_address: Address,
        amount: i128,
        _memo: String,
    ) -> Result<i128, ContractError> {
        source.require_auth();
        if amount <= 0 {
            return Err(ContractError::ZeroAmount);
        }

        let fee_bps: u32 = env.storage().instance().get(&DataKey::FeeBps).unwrap_or(0);
        let fee_amount = if fee_bps > 0 {
            (amount * fee_bps as i128) / 10000
        } else {
            0i128
        };

        if fee_amount > 0 {
            let accumulated: i128 = env
                .storage()
                .instance()
                .get(&DataKey::AccumulatedFees)
                .unwrap_or(0);
            env.storage()
                .instance()
                .set(&DataKey::AccumulatedFees, &(accumulated + fee_amount));
        }

        env.events().publish(
            (Symbol::new(&env, "funded"),),
            (source, target, amount, fee_amount),
        );

        Ok(fee_amount)
    }

    /// Withdraw accumulated fees. Pass `amount = 0` to withdraw everything.
    ///
    /// Errors:
    /// - `Unauthorized` — caller is not admin
    /// - `InsufficientFees` — requested amount exceeds balance
    pub fn withdraw_fees(
        env: Env,
        to: Address,
        token_address: Address,
        amount: i128,
    ) -> Result<i128, ContractError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ContractError::NotInitialized)?;
        admin.require_auth();

        let accumulated: i128 = env
            .storage()
            .instance()
            .get(&DataKey::AccumulatedFees)
            .unwrap_or(0);
        let withdraw_amount = if amount == 0 { accumulated } else { amount };
        if withdraw_amount > accumulated {
            return Err(ContractError::InsufficientFees);
        }

        env.storage()
            .instance()
            .set(&DataKey::AccumulatedFees, &(accumulated - withdraw_amount));

        env.events().publish(
            (Symbol::new(&env, "withdrawn"),),
            (to, token_address, withdraw_amount),
        );

        Ok(withdraw_amount)
    }

    pub fn route_from_exchange(
        env: Env,
        exchange: Address,
        target: Address,
        token_address: Address,
        amount: i128,
        memo: String,
    ) -> Result<i128, ContractError> {
        exchange.require_auth();
        Self::fund_c_address(env, exchange, target, token_address, amount, memo)
    }
}

mod test;

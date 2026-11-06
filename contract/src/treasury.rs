//! Self-funding treasury ("iMeme" mode).
//!
//! Sponsors stake NEAR into the swarm's treasury. The NEAR goes to a normal
//! NEAR staking pool. Only the staking *rewards* become the swarm's inference
//! budget, so the swarm pays for its own AI brain while the principal is never
//! spent.
//!
//! Flow:
//! 1. `sponsor_treasury` (payable): NEAR is staked with the pool. Principal is
//!    tracked per sponsor and in total.
//! 2. `sync_rewards` (anyone): reads our total balance in the pool. Anything
//!    above principal is rewards. `rewards_seen` only ever goes up.
//! 3. `spend_inference_budget` (reviewer): records inference paid from rewards.
//!    Capped by `rewards_seen - budget_spent`, so principal can't be touched.
//! 4. `harvest_rewards` (reviewer/owner): unstakes the spent amount.
//!    `collect_rewards` after the pool's unbonding (about 4 epochs) withdraws
//!    it and credits the reviewer account, which pays NEAR AI Cloud.
//!
//! State is kept under its own storage key so the escrow contract upgrades
//! without a state migration.

use crate::*;
use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};

const TREASURY_KEY: &[u8] = b"TREASURY";
const SPONSOR_PREFIX: &str = "SPONSOR:";
const MIN_SPONSOR: u128 = 10u128.pow(24); // 1 NEAR (pool minimum stake is about 1 NEAR)
const GAS_POOL: Gas = Gas::from_tgas(60);
const GAS_POOL_VIEW: Gas = Gas::from_tgas(10);

#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "near_sdk::borsh")]
struct TreasuryState {
    pool: Option<AccountId>,
    principal: u128,
    rewards_seen: u128,
    budget_spent: u128,
    /// Spent budget already unstaked (pending or collected).
    harvested: u128,
    /// Unstaked, waiting for the unbonding period.
    pending: u128,
    paid_to_reviewer: u128,
    last_sync_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct TreasuryView {
    pub pool: Option<AccountId>,
    pub principal: U128,
    pub rewards_seen: U128,
    pub budget_spent: U128,
    pub budget_available: U128,
    pub pending_unstake: U128,
    pub paid_to_reviewer: U128,
    pub last_sync_ms: u64,
}

#[ext_contract(ext_pool)]
#[allow(dead_code)]
trait StakingPool {
    fn deposit_and_stake(&mut self);
    fn unstake(&mut self, amount: U128);
    fn withdraw(&mut self, amount: U128);
    fn get_account_total_balance(&self, account_id: AccountId) -> U128;
}

fn load() -> TreasuryState {
    env::storage_read(TREASURY_KEY)
        .map(|b| TreasuryState::try_from_slice(&b).expect("bad treasury state"))
        .unwrap_or_default()
}

fn save(t: &TreasuryState) {
    env::storage_write(TREASURY_KEY, &borsh::to_vec(t).unwrap());
}

fn sponsor_key(a: &AccountId) -> Vec<u8> {
    format!("{SPONSOR_PREFIX}{a}").into_bytes()
}

fn sponsor_amount(a: &AccountId) -> u128 {
    env::storage_read(&sponsor_key(a))
        .map(|b| u128::try_from_slice(&b).unwrap_or(0))
        .unwrap_or(0)
}

#[near]
impl SwarmEscrow {
    /// Owner picks the staking pool once, before any NEAR is staked.
    pub fn set_staking_pool(&mut self, pool: AccountId) {
        require!(env::predecessor_account_id() == self.owner, "only owner");
        let mut t = load();
        require!(t.principal == 0 && t.pending == 0, "treasury already in use");
        t.pool = Some(pool);
        save(&t);
    }

    /// Stake NEAR into the swarm's treasury. Rewards fund inference;
    /// principal is never spent.
    #[payable]
    pub fn sponsor_treasury(&mut self) -> Promise {
        let amount = env::attached_deposit().as_yoctonear();
        require!(amount >= MIN_SPONSOR, "sponsor at least 1 NEAR");
        let pool = load().pool.expect("staking pool not set");
        let sponsor = env::predecessor_account_id();
        ext_pool::ext(pool)
            .with_attached_deposit(NearToken::from_yoctonear(amount))
            .with_static_gas(GAS_POOL)
            .deposit_and_stake()
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_CALLBACK)
                    .on_staked(sponsor, U128(amount)),
            )
    }

    #[private]
    pub fn on_staked(&mut self, sponsor: AccountId, amount: U128, #[callback_result] r: Result<(), PromiseError>) -> bool {
        if r.is_err() {
            // The pool refunds the attached deposit to us; give it back to the sponsor.
            self.credit(sponsor, Currency::Near, amount.0);
            env::log_str("treasury: staking failed, sponsor refunded to earnings");
            return false;
        }
        let mut t = load();
        t.principal += amount.0;
        save(&t);
        let prev = sponsor_amount(&sponsor);
        env::storage_write(&sponsor_key(&sponsor), &borsh::to_vec(&(prev + amount.0)).unwrap());
        env::log_str(&format!("treasury: {sponsor} staked {} yoctoNEAR", amount.0));
        true
    }

    /// Anyone may call. Reads the treasury's balance in the pool and records
    /// rewards = balance - principal.
    pub fn sync_rewards(&mut self) -> Promise {
        let pool = load().pool.expect("staking pool not set");
        ext_pool::ext(pool)
            .with_static_gas(GAS_POOL_VIEW)
            .get_account_total_balance(env::current_account_id())
            .then(Self::ext(env::current_account_id()).with_static_gas(GAS_CALLBACK).on_sync())
    }

    #[private]
    pub fn on_sync(&mut self, #[callback_result] bal: Result<U128, PromiseError>) -> U128 {
        let mut t = load();
        if let Ok(b) = bal {
            // Pool total (staked + unstaked) = principal + rewards not yet withdrawn.
            let rewards = (b.0 + t.paid_to_reviewer).saturating_sub(t.principal);
            if rewards > t.rewards_seen {
                t.rewards_seen = rewards;
            }
            t.last_sync_ms = env::block_timestamp_ms();
            save(&t);
        }
        U128(t.rewards_seen)
    }

    /// Reviewer records inference paid from staking rewards.
    pub fn spend_inference_budget(&mut self, amount: U128, note: String) {
        self.only_reviewer();
        require!(note.len() <= 200, "note too long");
        let mut t = load();
        require!(t.budget_spent + amount.0 <= t.rewards_seen, "not enough staking rewards");
        t.budget_spent += amount.0;
        save(&t);
        env::log_str(&format!("treasury: inference {} yoctoNEAR paid from rewards: {note}", amount.0));
    }

    /// Unstake the budget already spent so it can be paid out.
    pub fn harvest_rewards(&mut self) -> Promise {
        let caller = env::predecessor_account_id();
        require!(caller == self.reviewer || caller == self.owner, "only reviewer or owner");
        let mut t = load();
        let amount = t.budget_spent - t.harvested;
        require!(amount > 0, "nothing to harvest");
        require!(t.pending == 0, "previous harvest still unbonding");
        t.harvested += amount;
        t.pending = amount;
        save(&t);
        ext_pool::ext(t.pool.clone().expect("staking pool not set"))
            .with_static_gas(GAS_POOL)
            .unstake(U128(amount))
            .then(Self::ext(env::current_account_id()).with_static_gas(GAS_CALLBACK).on_unstaked(U128(amount)))
    }

    #[private]
    pub fn on_unstaked(&mut self, amount: U128, #[callback_result] r: Result<(), PromiseError>) -> bool {
        if r.is_err() {
            let mut t = load();
            t.harvested -= amount.0;
            t.pending = 0;
            save(&t);
            return false;
        }
        true
    }

    /// After unbonding, withdraw the harvested rewards to the reviewer.
    pub fn collect_rewards(&mut self) -> Promise {
        let t = load();
        require!(t.pending > 0, "nothing pending");
        ext_pool::ext(t.pool.clone().expect("staking pool not set"))
            .with_static_gas(GAS_POOL)
            .withdraw(U128(t.pending))
            .then(Self::ext(env::current_account_id()).with_static_gas(GAS_CALLBACK).on_withdrawn(U128(t.pending)))
    }

    #[private]
    pub fn on_withdrawn(&mut self, amount: U128, #[callback_result] r: Result<(), PromiseError>) -> bool {
        if r.is_err() {
            env::log_str("treasury: still unbonding, try collect_rewards later");
            return false;
        }
        let mut t = load();
        t.pending = 0;
        t.paid_to_reviewer += amount.0;
        save(&t);
        let reviewer = self.reviewer.clone();
        self.credit(reviewer, Currency::Near, amount.0);
        true
    }

    pub fn get_treasury(&self) -> TreasuryView {
        let t = load();
        TreasuryView {
            pool: t.pool,
            principal: U128(t.principal),
            rewards_seen: U128(t.rewards_seen),
            budget_spent: U128(t.budget_spent),
            budget_available: U128(t.rewards_seen - t.budget_spent),
            pending_unstake: U128(t.pending),
            paid_to_reviewer: U128(t.paid_to_reviewer),
            last_sync_ms: t.last_sync_ms,
        }
    }

    pub fn get_sponsor(&self, account_id: AccountId) -> U128 {
        U128(sponsor_amount(&account_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;

    const NEAR: u128 = 10u128.pow(24);

    fn acc(s: &str) -> AccountId {
        s.parse().unwrap()
    }

    fn ctx(pred: &str, dep: u128) {
        testing_env!(VMContextBuilder::new()
            .current_account_id(acc("swarm.testnet"))
            .predecessor_account_id(acc(pred))
            .attached_deposit(NearToken::from_yoctonear(dep))
            .block_timestamp(1_000_000_000)
            .build());
    }

    fn setup() -> SwarmEscrow {
        ctx("owner.testnet", 0);
        let mut c = SwarmEscrow::new(acc("owner.testnet"), acc("reviewer.testnet"), acc("wrap.testnet"), 60_000);
        c.set_staking_pool(acc("legends.pool.f863973.m0"));
        c
    }

    #[test]
    fn rewards_fund_inference_principal_untouched() {
        let mut c = setup();
        ctx("sponsor.testnet", 10 * NEAR);
        let _ = c.sponsor_treasury();
        ctx("swarm.testnet", 0);
        assert!(c.on_staked(acc("sponsor.testnet"), U128(10 * NEAR), Ok(())));
        assert_eq!(c.get_treasury().principal.0, 10 * NEAR);
        assert_eq!(c.get_sponsor(acc("sponsor.testnet")).0, 10 * NEAR);
        // Pool balance grows by 0.05 NEAR of rewards.
        c.on_sync(Ok(U128(10 * NEAR + NEAR / 20)));
        assert_eq!(c.get_treasury().budget_available.0, NEAR / 20);
        ctx("reviewer.testnet", 0);
        c.spend_inference_budget(U128(NEAR / 50), "review job 1".into());
        assert_eq!(c.get_treasury().budget_available.0, NEAR / 20 - NEAR / 50);
    }

    #[test]
    #[should_panic(expected = "not enough staking rewards")]
    fn cannot_spend_principal() {
        let mut c = setup();
        ctx("swarm.testnet", 0);
        c.on_staked(acc("sponsor.testnet"), U128(10 * NEAR), Ok(()));
        c.on_sync(Ok(U128(10 * NEAR)));
        ctx("reviewer.testnet", 0);
        c.spend_inference_budget(U128(1), "x".into());
    }

    #[test]
    fn failed_stake_refunds_sponsor() {
        let mut c = setup();
        ctx("swarm.testnet", 0);
        assert!(!c.on_staked(acc("sponsor.testnet"), U128(2 * NEAR), Err(PromiseError::Failed)));
        assert_eq!(c.get_earnings(acc("sponsor.testnet"), Currency::Near).0, 2 * NEAR);
        assert_eq!(c.get_treasury().principal.0, 0);
    }

    #[test]
    fn harvest_and_collect_pays_reviewer() {
        let mut c = setup();
        ctx("swarm.testnet", 0);
        c.on_staked(acc("sponsor.testnet"), U128(10 * NEAR), Ok(()));
        c.on_sync(Ok(U128(10 * NEAR + NEAR / 10)));
        ctx("reviewer.testnet", 0);
        c.spend_inference_budget(U128(NEAR / 20), "jobs".into());
        let _ = c.harvest_rewards();
        assert_eq!(c.get_treasury().pending_unstake.0, NEAR / 20);
        // Unstaked NEAR still counts in the pool total until withdrawn.
        ctx("swarm.testnet", 0);
        c.on_sync(Ok(U128(10 * NEAR + NEAR / 10)));
        assert_eq!(c.get_treasury().rewards_seen.0, NEAR / 10);
        let _ = c.collect_rewards();
        assert!(c.on_withdrawn(U128(NEAR / 20), Ok(())));
        // After withdrawal the pool total drops; rewards are not double counted or lost.
        c.on_sync(Ok(U128(10 * NEAR + NEAR / 10 - NEAR / 20)));
        assert_eq!(c.get_treasury().rewards_seen.0, NEAR / 10);
        assert_eq!(c.get_earnings(acc("reviewer.testnet"), Currency::Near).0, NEAR / 20);
        assert_eq!(c.get_treasury().paid_to_reviewer.0, NEAR / 20);
    }

    #[test]
    #[should_panic(expected = "only reviewer")]
    fn only_reviewer_spends() {
        let mut c = setup();
        ctx("random.testnet", 0);
        c.spend_inference_budget(U128(1), "x".into());
    }
}

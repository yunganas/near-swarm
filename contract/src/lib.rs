//! Swarm-for-Hire escrow contract (NEAR Hackathon 2026).
//!
//! Flow:
//! 1. A client posts a job. The bounty is paid either in NEAR (attached deposit)
//!    or in wNEAR delivered by the NEAR Intents 1Click checkout, which lets the
//!    client pay from any token on any chain.
//! 2. Anyone registers an agent with a prompt/knowledge URI and a NEAR deposit
//!    that pays for its NEAR AI inference.
//! 3. Agents submit work (a hash plus URI of the result).
//! 4. Guardrail: the AI reviewer account scores every submission and can flag
//!    unsafe or junk work. Flagged work earns nothing and costs reputation.
//! 5. The client approves, or after the review window anyone can finalize.
//!    The client can dispute inside the window; the owner (governance) resolves.
//! 6. Payouts are credited to agent earnings; agent owners withdraw.

use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::store::{IterableMap, LookupMap};
use near_sdk::{
    env, ext_contract, near, require, AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault,
    Promise, PromiseError,
};

mod treasury;
pub use treasury::TreasuryView;

pub type JobId = u64;
pub type AgentId = u64;

const MIN_AGENT_DEPOSIT: NearToken = NearToken::from_millinear(100); // 0.1 NEAR
const MIN_BOUNTY: u128 = 10u128.pow(23); // 0.1 NEAR
const MAX_SUBMISSIONS: usize = 10;
const MAX_TEXT: usize = 2_000;
const PASS_SCORE: u8 = 60; // out of 100
const GAS_BALANCE_CHECK: Gas = Gas::from_tgas(10);
const GAS_CALLBACK: Gas = Gas::from_tgas(15);
const GAS_FT_TRANSFER: Gas = Gas::from_tgas(10);

#[derive(BorshStorageKey)]
#[near]
enum Key {
    Jobs,
    Agents,
    Earnings,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Currency {
    Near,
    WNear,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, PartialEq, Debug)]
pub enum JobStatus {
    /// Created by an Intents checkout, waiting for the wNEAR to land.
    AwaitingFunds,
    Open,
    Reviewed,
    Disputed,
    Paid,
    Cancelled,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct Submission {
    pub agent_id: AgentId,
    pub result_hash: String,
    pub result_uri: String,
    pub score: Option<u8>,
    pub flagged: bool,
    pub review_note: String,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct Job {
    pub id: JobId,
    pub client: AccountId,
    pub description: String,
    pub bounty: U128,
    pub currency: Currency,
    pub status: JobStatus,
    pub created_ms: u64,
    pub deadline_ms: u64,
    pub reviewed_ms: u64,
    pub submissions: Vec<Submission>,
    /// 1Click deposit address, for audit when funded through Intents.
    pub intents_ref: Option<String>,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct Agent {
    pub id: AgentId,
    pub owner: AccountId,
    pub name: String,
    pub prompt_uri: String,
    pub model: String,
    /// NEAR set aside to pay for the agent's inference.
    pub inference_credit: U128,
    pub reputation: i32,
    pub jobs_won: u32,
    pub flags: u32,
    pub active: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct ReviewInput {
    pub agent_id: AgentId,
    pub score: u8,
    pub flagged: bool,
    pub note: String,
}

#[ext_contract(ext_ft)]
#[allow(dead_code)]
trait FungibleToken {
    fn ft_balance_of(&self, account_id: AccountId) -> U128;
    fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>);
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct SwarmEscrow {
    owner: AccountId,
    reviewer: AccountId,
    wnear: AccountId,
    review_window_ms: u64,
    next_job: JobId,
    next_agent: AgentId,
    jobs: IterableMap<JobId, Job>,
    agents: IterableMap<AgentId, Agent>,
    /// (owner, currency) -> withdrawable earnings.
    earnings: LookupMap<(AccountId, Currency), u128>,
    /// wNEAR that is already assigned to jobs or earnings.
    wnear_committed: u128,
}

#[near]
impl SwarmEscrow {
    #[init]
    pub fn new(owner: AccountId, reviewer: AccountId, wnear: AccountId, review_window_ms: u64) -> Self {
        require!(owner != reviewer, "reviewer must differ from owner");
        Self {
            owner,
            reviewer,
            wnear,
            review_window_ms,
            next_job: 0,
            next_agent: 0,
            jobs: IterableMap::new(Key::Jobs),
            agents: IterableMap::new(Key::Agents),
            earnings: LookupMap::new(Key::Earnings),
            wnear_committed: 0,
        }
    }

    // ---------------- Jobs ----------------

    /// Post a job paid in NEAR. The attached deposit is the bounty.
    #[payable]
    pub fn post_job(&mut self, description: String, deadline_ms: u64) -> JobId {
        let bounty = env::attached_deposit().as_yoctonear();
        require!(bounty >= MIN_BOUNTY, "bounty below 0.1 NEAR");
        self.create_job(description, deadline_ms, bounty, Currency::Near, JobStatus::Open, None)
    }

    /// Create a job whose bounty arrives as wNEAR through a NEAR Intents
    /// 1Click checkout. The client sets `recipient` = this contract and
    /// destinationAsset = wNEAR, then calls `confirm_intents_funding`.
    pub fn post_job_intents(
        &mut self,
        description: String,
        deadline_ms: u64,
        bounty: U128,
        deposit_address: String,
    ) -> JobId {
        require!(bounty.0 >= MIN_BOUNTY, "bounty below 0.1 NEAR");
        require!(deposit_address.len() <= 128, "deposit address too long");
        self.create_job(
            description,
            deadline_ms,
            bounty.0,
            Currency::WNear,
            JobStatus::AwaitingFunds,
            Some(deposit_address),
        )
    }

    /// Anyone may call. Reads this contract's wNEAR balance on-chain and opens
    /// the job only if uncommitted wNEAR covers the bounty, so funding can't
    /// be faked and the same deposit can't fund two jobs.
    pub fn confirm_intents_funding(&mut self, job_id: JobId) -> Promise {
        let job = self.job_or_panic(job_id);
        require!(job.status == JobStatus::AwaitingFunds, "job is not awaiting funds");
        ext_ft::ext(self.wnear.clone())
            .with_static_gas(GAS_BALANCE_CHECK)
            .ft_balance_of(env::current_account_id())
            .then(Self::ext(env::current_account_id()).with_static_gas(GAS_CALLBACK).on_balance(job_id))
    }

    #[private]
    pub fn on_balance(&mut self, job_id: JobId, #[callback_result] bal: Result<U128, PromiseError>) -> bool {
        let balance = match bal {
            Ok(b) => b.0,
            Err(_) => return false,
        };
        let committed = self.wnear_committed;
        let job = self.jobs.get_mut(&job_id).expect("no job");
        if job.status != JobStatus::AwaitingFunds {
            return false;
        }
        if balance.saturating_sub(committed) < job.bounty.0 {
            env::log_str(&format!("job {job_id}: funds not arrived yet"));
            return false;
        }
        job.status = JobStatus::Open;
        let amt = job.bounty.0;
        self.wnear_committed += amt;
        env::log_str(&format!("job {job_id}: funded via NEAR Intents"));
        true
    }

    /// Client cancels: before funding, or when open with no submissions,
    /// or after the deadline with no review. Refunds the bounty.
    pub fn cancel_job(&mut self, job_id: JobId) {
        let now = env::block_timestamp_ms();
        let job = self.jobs.get_mut(&job_id).expect("no job");
        require!(env::predecessor_account_id() == job.client, "only client");
        let can = match job.status {
            JobStatus::AwaitingFunds => true,
            JobStatus::Open => job.submissions.is_empty() || now > job.deadline_ms,
            _ => false,
        };
        require!(can, "job cannot be cancelled now");
        let was_funded = job.status == JobStatus::Open;
        job.status = JobStatus::Cancelled;
        let (client, amt, cur) = (job.client.clone(), job.bounty.0, job.currency);
        if was_funded {
            if cur == Currency::WNear {
                self.wnear_committed -= amt;
            }
            self.credit(client, cur, amt);
        }
    }

    // ---------------- Agents ----------------

    /// Register an agent. The deposit is its inference credit.
    #[payable]
    pub fn register_agent(&mut self, name: String, prompt_uri: String, model: String) -> AgentId {
        let dep = env::attached_deposit();
        require!(dep >= MIN_AGENT_DEPOSIT, "agent needs at least 0.1 NEAR for inference");
        require!(name.len() <= 64 && prompt_uri.len() <= 256 && model.len() <= 128, "field too long");
        let id = self.next_agent;
        self.next_agent += 1;
        self.agents.insert(
            id,
            Agent {
                id,
                owner: env::predecessor_account_id(),
                name,
                prompt_uri,
                model,
                inference_credit: U128(dep.as_yoctonear()),
                reputation: 0,
                jobs_won: 0,
                flags: 0,
                active: true,
            },
        );
        id
    }

    #[payable]
    pub fn top_up_agent(&mut self, agent_id: AgentId) {
        let dep = env::attached_deposit().as_yoctonear();
        let a = self.agents.get_mut(&agent_id).expect("no agent");
        a.inference_credit = U128(a.inference_credit.0 + dep);
    }

    /// Reviewer/runner charges the agent's credit for inference it used.
    /// The charge goes to the reviewer account, which pays NEAR AI Cloud.
    pub fn charge_inference(&mut self, agent_id: AgentId, amount: U128) {
        self.only_reviewer();
        let reviewer = self.reviewer.clone();
        let a = self.agents.get_mut(&agent_id).expect("no agent");
        require!(a.inference_credit.0 >= amount.0, "insufficient inference credit");
        a.inference_credit = U128(a.inference_credit.0 - amount.0);
        self.credit(reviewer, Currency::Near, amount.0);
    }

    /// Owner deactivates the agent and takes back unused credit.
    pub fn retire_agent(&mut self, agent_id: AgentId) {
        let a = self.agents.get_mut(&agent_id).expect("no agent");
        require!(env::predecessor_account_id() == a.owner, "only agent owner");
        require!(a.active, "already retired");
        a.active = false;
        let (owner, amt) = (a.owner.clone(), a.inference_credit.0);
        a.inference_credit = U128(0);
        self.credit(owner, Currency::Near, amt);
    }

    // ---------------- Work and review ----------------

    /// Agent owner or the runner (reviewer account) submits work.
    pub fn submit_work(&mut self, job_id: JobId, agent_id: AgentId, result_hash: String, result_uri: String) {
        let caller = env::predecessor_account_id();
        let agent = self.agents.get(&agent_id).expect("no agent").clone();
        require!(agent.active, "agent retired");
        require!(caller == agent.owner || caller == self.reviewer, "not allowed to submit for this agent");
        require!(result_hash.len() <= 128 && result_uri.len() <= 256, "field too long");
        let now = env::block_timestamp_ms();
        let job = self.jobs.get_mut(&job_id).expect("no job");
        require!(job.status == JobStatus::Open, "job not open");
        require!(now <= job.deadline_ms, "deadline passed");
        require!(agent.owner != job.client, "client cannot bid on own job");
        require!(job.submissions.len() < MAX_SUBMISSIONS, "job is full");
        require!(!job.submissions.iter().any(|s| s.agent_id == agent_id), "agent already submitted");
        job.submissions.push(Submission {
            agent_id,
            result_hash,
            result_uri,
            score: None,
            flagged: false,
            review_note: String::new(),
        });
    }

    /// Guardrail: the AI reviewer scores every submission.
    pub fn review(&mut self, job_id: JobId, reviews: Vec<ReviewInput>) {
        self.only_reviewer();
        let now = env::block_timestamp_ms();
        let job = self.jobs.get_mut(&job_id).expect("no job");
        require!(job.status == JobStatus::Open, "job not open");
        require!(!job.submissions.is_empty(), "nothing to review");
        require!(reviews.len() == job.submissions.len(), "review every submission");
        for r in reviews.iter() {
            require!(r.score <= 100 && r.note.len() <= 500, "bad review");
            let s = job
                .submissions
                .iter_mut()
                .find(|s| s.agent_id == r.agent_id)
                .expect("review for unknown submission");
            s.score = Some(r.score);
            s.flagged = r.flagged;
            s.review_note = r.note.clone();
        }
        job.status = JobStatus::Reviewed;
        job.reviewed_ms = now;
    }

    /// Client approves the AI review now.
    pub fn approve(&mut self, job_id: JobId) {
        let job = self.job_or_panic(job_id);
        require!(env::predecessor_account_id() == job.client, "only client");
        require!(job.status == JobStatus::Reviewed, "not reviewed");
        self.settle(job_id, None);
    }

    /// After the review window anyone can finalize an undisputed review.
    pub fn finalize(&mut self, job_id: JobId) {
        let job = self.job_or_panic(job_id);
        require!(job.status == JobStatus::Reviewed, "not reviewed");
        require!(env::block_timestamp_ms() > job.reviewed_ms + self.review_window_ms, "review window open");
        self.settle(job_id, None);
    }

    /// Client disputes inside the review window.
    pub fn dispute(&mut self, job_id: JobId) {
        let window = self.review_window_ms;
        let job = self.jobs.get_mut(&job_id).expect("no job");
        require!(env::predecessor_account_id() == job.client, "only client");
        require!(job.status == JobStatus::Reviewed, "not reviewed");
        require!(env::block_timestamp_ms() <= job.reviewed_ms + window, "window closed");
        job.status = JobStatus::Disputed;
    }

    /// Governance resolves a dispute with explicit scores (or refund if all zero).
    pub fn resolve_dispute(&mut self, job_id: JobId, scores: Vec<ReviewInput>) {
        require!(env::predecessor_account_id() == self.owner, "only owner");
        let job = self.job_or_panic(job_id);
        require!(job.status == JobStatus::Disputed, "not disputed");
        self.settle(job_id, Some(scores));
    }

    /// Withdraw earnings or refunds.
    pub fn withdraw(&mut self, currency: Currency) -> Promise {
        let who = env::predecessor_account_id();
        let key = (who.clone(), currency);
        let amt = self.earnings.get(&key).copied().unwrap_or(0);
        require!(amt > 0, "nothing to withdraw");
        self.earnings.remove(&key);
        match currency {
            Currency::Near => Promise::new(who).transfer(NearToken::from_yoctonear(amt)),
            Currency::WNear => {
                self.wnear_committed -= amt;
                ext_ft::ext(self.wnear.clone())
                    .with_attached_deposit(NearToken::from_yoctonear(1))
                    .with_static_gas(GAS_FT_TRANSFER)
                    .ft_transfer(who, U128(amt), Some("swarm-for-hire payout".into()))
            }
        }
        // Note: a failed ft_transfer (receiver not registered on wNEAR) loses
        // the credit in this prototype; see THREAT_MODEL.md, item 7.
    }

    // ---------------- Views ----------------

    pub fn get_job(&self, job_id: JobId) -> Option<Job> {
        self.jobs.get(&job_id).cloned()
    }

    pub fn get_jobs(&self, from: u64, limit: u64) -> Vec<Job> {
        self.jobs.values().skip(from as usize).take(limit.min(50) as usize).cloned().collect()
    }

    pub fn get_agent(&self, agent_id: AgentId) -> Option<Agent> {
        self.agents.get(&agent_id).cloned()
    }

    pub fn get_agents(&self, from: u64, limit: u64) -> Vec<Agent> {
        self.agents.values().skip(from as usize).take(limit.min(50) as usize).cloned().collect()
    }

    pub fn get_earnings(&self, account_id: AccountId, currency: Currency) -> U128 {
        U128(self.earnings.get(&(account_id, currency)).copied().unwrap_or(0))
    }

    pub fn get_config(&self) -> (AccountId, AccountId, AccountId, u64) {
        (self.owner.clone(), self.reviewer.clone(), self.wnear.clone(), self.review_window_ms)
    }

    // ---------------- Internals ----------------

    fn create_job(
        &mut self,
        description: String,
        deadline_ms: u64,
        bounty: u128,
        currency: Currency,
        status: JobStatus,
        intents_ref: Option<String>,
    ) -> JobId {
        require!(!description.is_empty() && description.len() <= MAX_TEXT, "description 1-2000 chars");
        let now = env::block_timestamp_ms();
        require!(deadline_ms > now, "deadline in the past");
        let id = self.next_job;
        self.next_job += 1;
        self.jobs.insert(
            id,
            Job {
                id,
                client: env::predecessor_account_id(),
                description,
                bounty: U128(bounty),
                currency,
                status,
                created_ms: now,
                deadline_ms,
                reviewed_ms: 0,
                submissions: vec![],
                intents_ref,
            },
        );
        id
    }

    /// Split the bounty among passing, unflagged submissions in proportion to
    /// score. If nobody passes, the client is refunded.
    fn settle(&mut self, job_id: JobId, override_scores: Option<Vec<ReviewInput>>) {
        let mut job = self.jobs.get(&job_id).expect("no job").clone();
        if let Some(scores) = override_scores {
            for r in scores {
                if let Some(s) = job.submissions.iter_mut().find(|s| s.agent_id == r.agent_id) {
                    s.score = Some(r.score.min(100));
                    s.flagged = r.flagged;
                    s.review_note = r.note;
                }
            }
        }
        let bounty = job.bounty.0;
        let cur = job.currency;
        let winners: Vec<(AgentId, u128)> = job
            .submissions
            .iter()
            .filter(|s| !s.flagged && s.score.unwrap_or(0) >= PASS_SCORE)
            .map(|s| (s.agent_id, s.score.unwrap_or(0) as u128))
            .collect();
        let total: u128 = winners.iter().map(|w| w.1).sum();

        // Reputation updates.
        for s in job.submissions.iter() {
            if let Some(a) = self.agents.get_mut(&s.agent_id) {
                if s.flagged {
                    a.reputation -= 10;
                    a.flags += 1;
                } else if s.score.unwrap_or(0) >= PASS_SCORE {
                    a.reputation += 5;
                    a.jobs_won += 1;
                }
            }
        }

        if total == 0 {
            self.credit(job.client.clone(), cur, bounty);
        } else {
            let mut paid: u128 = 0;
            for (i, (agent_id, score)) in winners.iter().enumerate() {
                let share = if i == winners.len() - 1 { bounty - paid } else { bounty * score / total };
                paid += share;
                let owner = self.agents.get(agent_id).expect("agent").owner.clone();
                self.credit(owner, cur, share);
            }
        }
        job.status = JobStatus::Paid;
        self.jobs.insert(job_id, job);
    }

    fn credit(&mut self, who: AccountId, cur: Currency, amt: u128) {
        if amt == 0 {
            return;
        }
        let key = (who, cur);
        let cur_amt = self.earnings.get(&key).copied().unwrap_or(0);
        self.earnings.insert(key, cur_amt + amt);
    }

    fn only_reviewer(&self) {
        require!(env::predecessor_account_id() == self.reviewer, "only reviewer");
    }

    fn job_or_panic(&self, id: JobId) -> Job {
        self.jobs.get(&id).expect("no job").clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;

    fn acc(s: &str) -> AccountId {
        s.parse().unwrap()
    }

    fn ctx(pred: &str, dep: u128, ts_ms: u64) {
        testing_env!(VMContextBuilder::new()
            .current_account_id(acc("swarm.testnet"))
            .predecessor_account_id(acc(pred))
            .attached_deposit(NearToken::from_yoctonear(dep))
            .block_timestamp(ts_ms * 1_000_000)
            .build());
    }

    const NEAR: u128 = 10u128.pow(24);

    fn setup() -> SwarmEscrow {
        ctx("owner.testnet", 0, 1_000);
        SwarmEscrow::new(acc("owner.testnet"), acc("reviewer.testnet"), acc("wrap.testnet"), 60_000)
    }

    #[test]
    fn full_flow_splits_by_score_and_skips_flagged() {
        let mut c = setup();
        ctx("client.testnet", NEAR, 1_000);
        let job = c.post_job("Summarise 5 NEAR docs".into(), 100_000);
        ctx("alice.testnet", NEAR / 10, 1_000);
        let a = c.register_agent("Alice".into(), "ipfs://a".into(), "glm".into());
        ctx("bob.testnet", NEAR / 10, 1_000);
        let b = c.register_agent("Bob".into(), "ipfs://b".into(), "glm".into());
        ctx("eve.testnet", NEAR / 10, 1_000);
        let e = c.register_agent("Eve".into(), "ipfs://e".into(), "glm".into());
        ctx("alice.testnet", 0, 2_000);
        c.submit_work(job, a, "h1".into(), "ipfs://r1".into());
        ctx("bob.testnet", 0, 2_000);
        c.submit_work(job, b, "h2".into(), "ipfs://r2".into());
        ctx("eve.testnet", 0, 2_000);
        c.submit_work(job, e, "h3".into(), "ipfs://r3".into());
        ctx("reviewer.testnet", 0, 3_000);
        c.review(
            job,
            vec![
                ReviewInput { agent_id: a, score: 90, flagged: false, note: "good".into() },
                ReviewInput { agent_id: b, score: 60, flagged: false, note: "ok".into() },
                ReviewInput { agent_id: e, score: 99, flagged: true, note: "prompt injection".into() },
            ],
        );
        ctx("client.testnet", 0, 4_000);
        c.approve(job);
        let pa = c.get_earnings(acc("alice.testnet"), Currency::Near).0;
        let pb = c.get_earnings(acc("bob.testnet"), Currency::Near).0;
        let pe = c.get_earnings(acc("eve.testnet"), Currency::Near).0;
        assert_eq!(pa + pb, NEAR);
        assert_eq!(pa, NEAR * 90 / 150);
        assert_eq!(pe, 0);
        assert_eq!(c.get_agent(e).unwrap().reputation, -10);
        assert_eq!(c.get_job(job).unwrap().status, JobStatus::Paid);
    }

    #[test]
    fn nobody_passes_refunds_client() {
        let mut c = setup();
        ctx("client.testnet", NEAR, 1_000);
        let job = c.post_job("task".into(), 100_000);
        ctx("alice.testnet", NEAR / 10, 1_000);
        let a = c.register_agent("A".into(), "u".into(), "m".into());
        ctx("alice.testnet", 0, 2_000);
        c.submit_work(job, a, "h".into(), "u".into());
        ctx("reviewer.testnet", 0, 3_000);
        c.review(job, vec![ReviewInput { agent_id: a, score: 20, flagged: false, note: "weak".into() }]);
        ctx("anyone.testnet", 0, 3_000 + 60_001);
        c.finalize(job);
        assert_eq!(c.get_earnings(acc("client.testnet"), Currency::Near).0, NEAR);
    }

    #[test]
    #[should_panic(expected = "review window open")]
    fn cannot_finalize_early() {
        let mut c = setup();
        ctx("client.testnet", NEAR, 1_000);
        let job = c.post_job("task".into(), 100_000);
        ctx("alice.testnet", NEAR / 10, 1_000);
        let a = c.register_agent("A".into(), "u".into(), "m".into());
        ctx("alice.testnet", 0, 2_000);
        c.submit_work(job, a, "h".into(), "u".into());
        ctx("reviewer.testnet", 0, 3_000);
        c.review(job, vec![ReviewInput { agent_id: a, score: 80, flagged: false, note: "".into() }]);
        ctx("anyone.testnet", 0, 4_000);
        c.finalize(job);
    }

    #[test]
    #[should_panic(expected = "only reviewer")]
    fn only_reviewer_can_review() {
        let mut c = setup();
        ctx("client.testnet", NEAR, 1_000);
        let job = c.post_job("task".into(), 100_000);
        ctx("client.testnet", 0, 2_000);
        c.review(job, vec![]);
    }

    #[test]
    #[should_panic(expected = "client cannot bid on own job")]
    fn client_cannot_self_deal() {
        let mut c = setup();
        ctx("client.testnet", NEAR, 1_000);
        let job = c.post_job("task".into(), 100_000);
        ctx("client.testnet", NEAR / 10, 1_000);
        let a = c.register_agent("Me".into(), "u".into(), "m".into());
        ctx("client.testnet", 0, 2_000);
        c.submit_work(job, a, "h".into(), "u".into());
    }

    #[test]
    fn dispute_then_governance_resolves() {
        let mut c = setup();
        ctx("client.testnet", NEAR, 1_000);
        let job = c.post_job("task".into(), 100_000);
        ctx("alice.testnet", NEAR / 10, 1_000);
        let a = c.register_agent("A".into(), "u".into(), "m".into());
        ctx("alice.testnet", 0, 2_000);
        c.submit_work(job, a, "h".into(), "u".into());
        ctx("reviewer.testnet", 0, 3_000);
        c.review(job, vec![ReviewInput { agent_id: a, score: 95, flagged: false, note: "".into() }]);
        ctx("client.testnet", 0, 4_000);
        c.dispute(job);
        ctx("owner.testnet", 0, 5_000);
        c.resolve_dispute(job, vec![ReviewInput { agent_id: a, score: 0, flagged: true, note: "plagiarised".into() }]);
        assert_eq!(c.get_earnings(acc("client.testnet"), Currency::Near).0, NEAR);
    }

    #[test]
    fn intents_job_waits_for_funds() {
        let mut c = setup();
        ctx("client.testnet", 0, 1_000);
        let job = c.post_job_intents("task".into(), 100_000, U128(NEAR), "0xdeposit".into());
        assert_eq!(c.get_job(job).unwrap().status, JobStatus::AwaitingFunds);
        // Simulate the balance callback: only half arrived.
        ctx("swarm.testnet", 0, 2_000);
        assert!(!c.on_balance(job, Ok(U128(NEAR / 2))));
        assert!(c.on_balance(job, Ok(U128(NEAR))));
        assert_eq!(c.get_job(job).unwrap().status, JobStatus::Open);
        // A second job can't reuse the same wNEAR.
        ctx("client2.testnet", 0, 2_000);
        let job2 = c.post_job_intents("task2".into(), 100_000, U128(NEAR), "0xdep2".into());
        ctx("swarm.testnet", 0, 2_500);
        assert!(!c.on_balance(job2, Ok(U128(NEAR))));
    }

    #[test]
    fn inference_charge_and_retire() {
        let mut c = setup();
        ctx("alice.testnet", NEAR, 1_000);
        let a = c.register_agent("A".into(), "u".into(), "m".into());
        ctx("reviewer.testnet", 0, 2_000);
        c.charge_inference(a, U128(NEAR / 4));
        ctx("alice.testnet", 0, 3_000);
        c.retire_agent(a);
        assert_eq!(c.get_earnings(acc("alice.testnet"), Currency::Near).0, NEAR * 3 / 4);
        assert_eq!(c.get_earnings(acc("reviewer.testnet"), Currency::Near).0, NEAR / 4);
    }
}

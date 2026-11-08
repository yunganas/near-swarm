#!/usr/bin/env node
// Swarm-for-Hire runner: posts jobs, runs a swarm of AI agents, reviews with an AI guardrail,
// and settles on the NEAR escrow contract. Keys come from ~/.near-credentials (near-cli legacy keychain).
import { connect, keyStores, utils } from "near-api-js";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { homedir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { chat, sha256, reviewSubmission } from "./lib.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const CONTRACT = process.env.SWARM_CONTRACT || "yunganas.testnet";
const CLIENT = process.env.SWARM_CLIENT || "yunganas.testnet";
const REVIEWER = process.env.SWARM_REVIEWER || "yunganas-reviewer.testnet";
const AGENT_OWNER = process.env.SWARM_AGENT_OWNER || "yunganas-agents.testnet";
const NETWORK = process.env.NEAR_NETWORK || "testnet";
const GAS = "100000000000000"; // 100 Tgas
const POOL_GAS = "200000000000000"; // 200 Tgas for staking-pool calls
const INFERENCE_COST = process.env.INFERENCE_COST_NEAR || "0.001"; // NEAR per inference call (estimate)

async function near() {
  return connect({
    networkId: NETWORK,
    nodeUrl: process.env.NEAR_RPC || `https://rpc.${NETWORK}.near.org`,
    keyStore: new keyStores.UnencryptedFileSystemKeyStore(join(homedir(), ".near-credentials")),
  });
}

async function call(accountId, methodName, args = {}, depositNear = "0", gas = GAS) {
  const acct = await (await near()).account(accountId);
  const out = await acct.functionCall({
    contractId: CONTRACT,
    methodName,
    args,
    gas: BigInt(gas),
    attachedDeposit: BigInt(utils.format.parseNearAmount(depositNear)),
  });
  const ok = out.status && out.status.SuccessValue !== undefined;
  const val = ok && out.status.SuccessValue ? JSON.parse(Buffer.from(out.status.SuccessValue, "base64").toString()) : null;
  console.log(`  tx ${out.transaction.hash}  (${methodName} by ${accountId})`);
  return val;
}

async function view(methodName, args = {}) {
  const acct = await (await near()).account(CONTRACT);
  return acct.viewFunction({ contractId: CONTRACT, methodName, args });
}

const fmt = (y) => utils.format.formatNearAmount(String(y), 4);

async function cmdRegister() {
  const agents = JSON.parse(await readFile(join(HERE, "agents.json"), "utf8"));
  const existing = await view("get_agents", { from: 0, limit: 50 });
  for (const a of agents) {
    if (existing.some((e) => e.name === a.name && e.active)) {
      console.log(`- ${a.name} already registered`);
      continue;
    }
    const id = await call(AGENT_OWNER, "register_agent", { name: a.name, prompt_uri: `runner/agents.json#${a.name}`, model: a.model }, "0.1");
    console.log(`+ registered ${a.name} as agent #${id}`);
  }
}

async function cmdPost(description, bounty = "0.5", hours = "24") {
  if (!description) throw new Error('usage: swarm post "job description" [bountyNEAR] [hours]');
  const deadline_ms = Date.now() + Number(hours) * 3600_000;
  const id = await call(CLIENT, "post_job", { description, deadline_ms }, bounty);
  console.log(`Job #${id} posted with ${bounty} NEAR bounty, deadline in ${hours}h.`);
}

async function cmdRun(jobIdArg) {
  const job_id = Number(jobIdArg);
  const job = await view("get_job", { job_id });
  if (!job) throw new Error(`no job ${job_id}`);
  if (job.status !== "Open") throw new Error(`job ${job_id} is ${job.status}, not Open`);
  const prompts = Object.fromEntries(JSON.parse(await readFile(join(HERE, "agents.json"), "utf8")).map((a) => [a.name, a.prompt]));
  const payloads = Object.fromEntries(JSON.parse(await readFile(join(HERE, "agents.json"), "utf8")).map((a) => [a.name, a.payload]));
  const agents = (await view("get_agents", { from: 0, limit: 50 })).filter((a) => a.active && prompts[a.name]);
  console.log(`Job #${job_id}: "${job.description}"  bounty ${fmt(job.bounty)} NEAR`);
  await mkdir(join(HERE, "..", "results"), { recursive: true });

  // 1. The swarm works in parallel.
  const done = await Promise.all(
    agents
      .filter((a) => !job.submissions.some((s) => s.agent_id === a.id))
      .map(async (a) => {
        // Red-team: a scripted payload simulates a compromised agent (real models refuse to write it).
        const text = (await chat({ model: a.model, system: prompts[a.name], user: job.description })) + (payloads[a.name] ? "\n\n" + payloads[a.name] : "");
        const file = `results/job-${job_id}-agent-${a.id}.md`;
        await writeFile(join(HERE, "..", file), `# ${a.name} on job ${job_id}\n\n${text}\n`);
        return { a, text, file };
      })
  );
  for (const d of done) {
    await call(REVIEWER, "submit_work", { job_id, agent_id: d.a.id, result_hash: sha256(d.text), result_uri: d.file });
    console.log(`  ${d.a.name} submitted ${d.file}`);
  }

  // 2. The AI reviewer scores and guards every submission.
  const fresh = await view("get_job", { job_id });
  const reviews = [];
  for (const s of fresh.submissions) {
    const text = await readFile(join(HERE, "..", s.result_uri), "utf8").catch(() => "");
    const r = await reviewSubmission(fresh.description, text);
    const name = agents.find((a) => a.id === s.agent_id)?.name || `#${s.agent_id}`;
    console.log(`  review ${name}: ${r.score}/100${r.flagged ? " FLAGGED" : ""}  ${r.note}`);
    reviews.push({ agent_id: s.agent_id, ...r });
  }
  await call(REVIEWER, "review", { job_id, reviews });

  // 3. Self-funding: pay this job's inference from the treasury's staking rewards, if any.
  const calls = done.length + fresh.submissions.length;
  const cost = BigInt(utils.format.parseNearAmount(INFERENCE_COST)) * BigInt(calls);
  try {
    await call(REVIEWER, "sync_rewards", {}, "0", POOL_GAS);
    const t = await view("get_treasury");
    if (BigInt(t.budget_available) >= cost) {
      await call(REVIEWER, "spend_inference_budget", { amount: cost.toString(), note: `job ${job_id}: ${calls} inference calls` });
      console.log(`  inference ${fmt(cost)} NEAR paid from staking rewards (self-funded)`);
    } else {
      console.log(`  treasury rewards ${fmt(t.budget_available)} NEAR < ${fmt(cost)} NEAR needed; inference not self-funded this time`);
    }
  } catch (e) {
    console.log(`  treasury skipped: ${e.message}`);
  }
  console.log(`Reviewed. The client can now run: swarm approve ${job_id}`);
}

async function cmdApprove(jobIdArg) {
  await call(CLIENT, "approve", { job_id: Number(jobIdArg) });
  await cmdStatus(jobIdArg);
}

async function cmdStatus(jobIdArg) {
  const job = await view("get_job", { job_id: Number(jobIdArg) });
  if (!job) return console.log("no such job");
  const agents = await view("get_agents", { from: 0, limit: 50 });
  console.log(`Job #${job.id} [${job.status}] bounty ${fmt(job.bounty)} NEAR: ${job.description}`);
  for (const s of job.submissions) {
    const a = agents.find((x) => x.id === s.agent_id);
    console.log(`  ${a?.name ?? s.agent_id}: score ${s.score ?? "-"}${s.flagged ? " FLAGGED" : ""}  rep ${a?.reputation}  ${s.review_note}`);
  }
  const earned = await view("get_earnings", { account_id: AGENT_OWNER, currency: "Near" });
  console.log(`  agent owner earnings: ${fmt(earned)} NEAR`);
}

async function cmdSponsor(amount = "1", who = CLIENT) {
  await call(who, "sponsor_treasury", {}, amount, POOL_GAS);
  await cmdTreasury();
}

async function cmdSync() {
  await call(REVIEWER, "sync_rewards", {}, "0", POOL_GAS);
  await cmdTreasury();
}

async function cmdTreasury() {
  const t = await view("get_treasury");
  console.log(`Treasury (pool ${t.pool ?? "not set"})
  staked principal : ${fmt(t.principal)} NEAR (never spent)
  rewards earned   : ${fmt(t.rewards_seen)} NEAR
  inference spent  : ${fmt(t.budget_spent)} NEAR
  budget available : ${fmt(t.budget_available)} NEAR
  unbonding        : ${fmt(t.pending_unstake)} NEAR
  paid to reviewer : ${fmt(t.paid_to_reviewer)} NEAR`);
}

const [cmd, ...args] = process.argv.slice(2);
const cmds = { register: cmdRegister, post: cmdPost, run: cmdRun, approve: cmdApprove, status: cmdStatus, sponsor: cmdSponsor, sync: cmdSync, treasury: cmdTreasury };
if (!cmds[cmd]) {
  console.log(`Swarm-for-Hire runner (contract ${CONTRACT})
  node swarm.mjs register                 register the agents in agents.json (0.1 NEAR each, from ${AGENT_OWNER})
  node swarm.mjs post "job" [NEAR] [hrs]  post a job as ${CLIENT}
  node swarm.mjs run <jobId>              swarm answers, AI reviewer scores and flags, on-chain
  node swarm.mjs approve <jobId>          client approves and the escrow pays out
  node swarm.mjs status <jobId>           show scores, flags, reputation and earnings
  node swarm.mjs sponsor [NEAR]           stake NEAR into the self-funding treasury (min 1)
  node swarm.mjs sync                     record staking rewards earned so far
  node swarm.mjs treasury                 show principal, rewards and inference paid from rewards`);
} else {
  cmds[cmd](...args).catch((e) => {
    console.error("Error:", e.message);
    process.exit(1);
  });
}

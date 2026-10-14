"""One PPO update of the goal network from rollouts (REIMAGINED.md L1.3-L1.4).

Reads the battles `examples/rollout` writes, computes rewards from each
battle's ledger, and updates the policy (the goal scorer the engine runs) and
a value function (only ever run here). Writes the new policy as the same JSON
the engine reads, probe included.

    uv run ppo.py ../target/rl/iter0.jsonl --net ../target/goal_net.json \\
        --out ../target/rl/net1.json

**The engine has no opinion about rewards.** It records what happened — every
crew's condition and both sides' scores, every round — and this file decides
what that was worth, so the weights below can be changed and re-run without a
rebuild. They are REIMAGINED.md's rule in numbers: the mission first (the
battle's outcome and its objective score, shared by the whole side), and the
crew's own survival second (charged to her alone).

**A decision is an option, not a step.** A cadet chooses a goal and carries it
for several rounds, so decision i is credited with the rewards of the rounds
from its own up to the next decision she takes (or the end of the battle),
discounted per round, and bootstrapped from the value at her next decision:
the semi-Markov form of the TD error, and GAE over each crew's decisions.
"""

from __future__ import annotations

import argparse
import json
import pathlib
from collections import defaultdict

import jax
import jax.numpy as jnp
import numpy as np
import optax

from imitate import exact_scores, scores

jax.config.update("jax_default_matmul_precision", "highest")


# --- rewards ----------------------------------------------------------------


def round_rewards(battle, w):
    """Per round, for the learner's side: the team's reward, and each of its
    crews' own share. Round k's reward is what changed between ledger rounds
    k-1 and k."""
    learner = battle["learner"]
    rounds = battle["rounds"]
    left = defaultdict(dict)  # round -> unit -> left
    full, side = {}, {}
    for e in battle["ledger"]:
        left[e["round"]][e["unit"]] = e["left"]
        full[e["unit"]] = e["full"]
        side[e["unit"]] = e["side"]
    ours = [u for u in full if side[u] == learner]
    theirs = [u for u in full if side[u] != learner]
    our_full = max(1, sum(full[u] for u in ours))
    their_full = max(1, sum(full[u] for u in theirs))
    score = {r: (s0, s1) for r, s0, s1 in battle["scores"]}

    def margin(r):
        s0, s1 = score[r]
        return (s0 - s1) if learner == 0 else (s1 - s0)

    team, own = {}, {}
    for k in range(1, rounds + 1):
        dealt = sum(left[k - 1][u] - left[k][u] for u in theirs) / their_full
        taken = sum(left[k - 1][u] - left[k][u] for u in ours) / our_full
        r = w["dealt"] * dealt - w["taken"] * taken + w["score"] * (margin(k) - margin(k - 1))
        if k == rounds:
            winner = battle["winner"]
            r += w["win"] * (0 if winner is None else (1 if winner == learner else -1))
        team[k] = r
        own[k] = {u: -w["self"] * (left[k - 1][u] - left[k][u]) / max(1, full[u]) for u in ours}
    return team, own


def transitions(battles, w, gamma):
    """Every learner decision, with its reward over the rounds it governed,
    the discount to the next decision, and whether one follows."""
    rows = []
    for b in battles:
        team, own = round_rewards(b, w)
        mine = [d for d in b["decisions"] if d["side"] == b["learner"] and d.get("logp") is not None]
        by_unit = defaultdict(list)
        for d in mine:
            by_unit[d["unit"]].append(d)
        for unit, ds in by_unit.items():
            ds.sort(key=lambda d: d["round"])
            for i, d in enumerate(ds):
                start = d["round"]
                end = ds[i + 1]["round"] if i + 1 < len(ds) else b["rounds"] + 1
                reward = sum(
                    gamma ** (k - start) * (team[k] + own[k].get(unit, 0.0))
                    for k in range(start, min(end, b["rounds"] + 1))
                )
                rows.append({
                    "battle": b["game"], "unit": unit, "index": i,
                    "context": d["context"], "candidates": d["candidates"],
                    "chosen": d["chosen"], "logp": d["logp"],
                    "reward": reward, "discount": gamma ** (end - start),
                    "last": i + 1 == len(ds),
                })
    return rows


# --- the networks -----------------------------------------------------------


def load_policy(path):
    net = json.loads(pathlib.Path(path).read_text())
    params = [{"w": jnp.asarray(l["w"], jnp.float32), "b": jnp.asarray(l["b"], jnp.float32)} for l in net["layers"]]
    return net, params


def value_init(key, width, hidden=(64, 64)):
    sizes = [width, *hidden, 1]
    layers = []
    for fan_in, fan_out in zip(sizes[:-1], sizes[1:]):
        key, sub = jax.random.split(key)
        layers.append({"w": jax.random.normal(sub, (fan_in, fan_out)) * jnp.sqrt(2.0 / fan_in),
                       "b": jnp.zeros(fan_out)})
    return layers


def value_input(x, mask):
    """What the critic sees: her context and the candidates pooled — the mean
    and the max of every candidate feature — so it knows what was on offer
    without a slot per candidate."""
    m = mask[..., None]
    count = jnp.maximum(m.sum(1), 1.0)
    mean = (x * m).sum(1) / count
    big = jnp.where(m, x, -1e9).max(1)
    big = jnp.where(count > 0, big, 0.0)
    return jnp.concatenate([mean, big], axis=-1)


def value(params, v):
    h = v
    for layer in params[:-1]:
        h = jax.nn.relu(h @ layer["w"] + layer["b"])
    return (h @ params[-1]["w"] + params[-1]["b"])[..., 0]


# --- the update -------------------------------------------------------------


def pad(rows, mean, std):
    k = max(len(r["candidates"]) for r in rows)
    width = len(rows[0]["context"]) + len(rows[0]["candidates"][0])
    x = np.zeros((len(rows), k, width), np.float32)
    mask = np.zeros((len(rows), k), bool)
    for i, r in enumerate(rows):
        for j, c in enumerate(r["candidates"]):
            x[i, j] = np.concatenate([r["context"], c])
            mask[i, j] = True
    xs = np.where(mask[..., None], (x - mean) / std, 0.0).astype(np.float32)
    return xs, mask


def advantages(rows, v, lam):
    """GAE over each crew's decisions in each battle, semi-Markov: the
    discount between two decisions is the rounds between them."""
    adv = np.zeros(len(rows), np.float32)
    groups = defaultdict(list)
    for i, r in enumerate(rows):
        groups[(r["battle"], r["unit"])].append(i)
    for idx in groups.values():
        idx.sort(key=lambda i: rows[i]["index"])
        running = 0.0
        for n in reversed(range(len(idx))):
            i = idx[n]
            nxt = 0.0 if rows[i]["last"] else v[idx[n + 1]]
            delta = rows[i]["reward"] + rows[i]["discount"] * nxt - v[i]
            running = delta + (0.0 if rows[i]["last"] else rows[i]["discount"] * lam * running)
            adv[i] = running
    return adv


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("rollouts", type=pathlib.Path)
    ap.add_argument("--net", type=pathlib.Path, required=True, help="the policy the rollouts were played with")
    ap.add_argument("--value", type=pathlib.Path, help="the critic from the last update, if any")
    ap.add_argument("--out", type=pathlib.Path, required=True)
    ap.add_argument("--temperature", type=float, default=1.0)
    ap.add_argument("--gamma", type=float, default=0.9, help="discount per round")
    ap.add_argument("--lam", type=float, default=0.95)
    ap.add_argument("--epochs", type=int, default=4)
    ap.add_argument("--batch", type=int, default=256)
    ap.add_argument("--lr", type=float, default=3e-4)
    ap.add_argument("--clip", type=float, default=0.2)
    ap.add_argument("--entropy", type=float, default=0.01)
    ap.add_argument("--seed", type=int, default=0)
    for name, default in [("dealt", 1.0), ("taken", 1.0), ("score", 0.05), ("win", 1.0), ("self", 0.5)]:
        ap.add_argument(f"--w-{name}", type=float, default=default)
    args = ap.parse_args()
    w = {k: getattr(args, f"w_{k}") for k in ("dealt", "taken", "score", "win", "self")}

    battles = [json.loads(l) for l in args.rollouts.open()]
    rows = transitions(battles, w, args.gamma)
    net, policy = load_policy(args.net)
    mean, std = np.asarray(net["mean"], np.float32), np.asarray(net["std"], np.float32)
    xs, mask = pad(rows, mean, std)
    chosen = np.array([r["chosen"] for r in rows], np.int32)
    old_logp = np.array([r["logp"] for r in rows], np.float32)
    T = args.temperature

    key = jax.random.key(args.seed)
    vin_width = 2 * xs.shape[-1]
    if args.value and args.value.exists():
        critic = [{"w": jnp.asarray(l["w"]), "b": jnp.asarray(l["b"])} for l in json.loads(args.value.read_text())["layers"]]
    else:
        critic = value_init(key, vin_width)
    vin = value_input(jnp.asarray(xs), jnp.asarray(mask))
    v_now = np.asarray(value(critic, vin))
    adv = advantages(rows, v_now, args.lam)
    returns = adv + v_now
    # How much of what happened the critic saw coming: near zero is a critic
    # that has learned nothing, and then every advantage is noise.
    explained = 1.0 - np.var(returns - v_now) / max(np.var(returns), 1e-8)
    adv_n = (adv - adv.mean()) / (adv.std() + 1e-8)

    params = {"policy": policy, "critic": critic}
    opt = optax.chain(optax.clip_by_global_norm(1.0), optax.adam(args.lr))
    state = opt.init(params)

    def loss_fn(p, x, m, vi, c, olp, a, ret):
        s = jnp.where(m, scores(p["policy"], x) / T, -1e9)
        logp_all = jax.nn.log_softmax(s, axis=-1)
        logp = jnp.take_along_axis(logp_all, c[:, None], axis=-1)[:, 0]
        ratio = jnp.exp(logp - olp)
        clipped = jnp.clip(ratio, 1 - args.clip, 1 + args.clip)
        pg = -jnp.minimum(ratio * a, clipped * a).mean()
        probs = jnp.exp(logp_all) * m
        ent = -(jnp.where(m, probs * logp_all, 0.0)).sum(-1).mean()
        vl = ((value(p["critic"], vi) - ret) ** 2).mean()
        return pg + 0.5 * vl - args.entropy * ent, (pg, vl, ent, jnp.abs(ratio - 1).mean())

    @jax.jit
    def step(p, st, *batch):
        (loss, aux), g = jax.value_and_grad(loss_fn, has_aux=True)(p, *batch)
        upd, st = opt.update(g, st, p)
        return optax.apply_updates(p, upd), st, aux

    data = [jnp.asarray(a) for a in (xs, mask)] + [vin] + [jnp.asarray(a) for a in (chosen, old_logp, adv_n, returns)]
    rng = np.random.default_rng(args.seed)
    for epoch in range(args.epochs):
        order = rng.permutation(len(rows))
        for i in range(0, len(order), args.batch):
            idx = order[i : i + args.batch]
            params, state, aux = step(params, state, *(d[idx] for d in data))
    pg, vl, ent, drift = (float(a) for a in aux)

    wins = sum(1 for b in battles if b["winner"] == b["learner"])
    losses = sum(1 for b in battles if b["winner"] is not None and b["winner"] != b["learner"])
    print(f"{len(battles)} battles (learner {wins}-{losses}), {len(rows)} decisions, "
          f"mean return {returns.mean():+.3f}; pg {pg:+.4f} value {vl:.4f} explained {explained:+.2f} "
          f"entropy {ent:.3f} ratio drift {drift:.3f}")

    # The policy, as the engine reads it, with a probe computed exactly.
    out = dict(net)
    out["layers"] = [{"w": np.asarray(l["w"]).tolist(), "b": np.asarray(l["b"]).tolist()} for l in params["policy"]]
    rows_x = xs[mask][:: max(1, mask.sum() // 16)][:16]
    raw = rows_x * std + mean
    got = exact_scores(params["policy"], rows_x)
    width_c = len(net["context"])
    out["probe"] = [{"context": r[:width_c].tolist(), "candidate": r[width_c:].tolist(), "score": float(g)}
                    for r, g in zip(raw, got)]
    out["trained_on"] = {"ppo_from": str(args.net), "rollouts": str(args.rollouts), "decisions": len(rows),
                         "weights": w, "gamma": args.gamma}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(out))
    critic_path = args.out.with_suffix(".critic.json")
    critic_path.write_text(json.dumps({"layers": [{"w": np.asarray(l["w"]).tolist(), "b": np.asarray(l["b"]).tolist()}
                                                  for l in params["critic"]]}))


if __name__ == "__main__":
    main()

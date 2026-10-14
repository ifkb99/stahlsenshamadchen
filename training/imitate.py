"""Train a goal-scoring network to imitate the utility chooser (REIMAGINED.md L0.4).

Reads the JSON lines `examples/goal_dataset` writes, and the schema beside
them. Each decision is a context row and a row per candidate; the network
scores ``concat(context, candidate)`` for every candidate and is trained with
cross-entropy against the one the teacher chose. Scoring candidates rather
than predicting a slot is what lets a new kind of goal be a fine-tune later.

Writes the network as JSON — feature names, the standardisation, and the
layers — which ``tactics_core::ai::learned`` reads, so the game runs it in
plain Rust with no dependency.

    uv run imitate.py ../target/goal_dataset/train.jsonl --out ../target/goal_net.json

Validation is by battle, not by decision: decisions in one battle are
correlated, and splitting them would let the network see the answer.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import time

import jax
import jax.numpy as jnp
import numpy as np
import optax


def load(path: pathlib.Path):
    schema = json.loads(path.with_suffix(".schema.json").read_text())
    games, contexts, candidates, chosen = [], [], [], []
    with path.open() as f:
        for line in f:
            r = json.loads(line)
            games.append(r["game"])
            contexts.append(r["context"])
            candidates.append(r["candidates"])
            chosen.append(r["chosen"])
    n = len(chosen)
    k = max(len(c) for c in candidates)
    width_c = len(schema["context"])
    width_g = len(schema["candidate"])
    x = np.zeros((n, k, width_c + width_g), np.float32)
    mask = np.zeros((n, k), bool)
    for i, (ctx, rows) in enumerate(zip(contexts, candidates)):
        for j, row in enumerate(rows):
            x[i, j, :width_c] = ctx
            x[i, j, width_c:] = row
            mask[i, j] = True
    return schema, np.array(games), x, mask, np.array(chosen, np.int32)


def standardisation(x, mask):
    rows = x[mask]
    mean = rows.mean(axis=0)
    std = rows.std(axis=0)
    # A feature that never varies in the data (an order verb nobody was
    # given) is passed through centred rather than divided by zero.
    std = np.where(std < 1e-6, 1.0, std)
    return mean.astype(np.float32), std.astype(np.float32)


def init(key, sizes):
    layers = []
    for fan_in, fan_out in zip(sizes[:-1], sizes[1:]):
        key, sub = jax.random.split(key)
        w = jax.random.normal(sub, (fan_in, fan_out)) * jnp.sqrt(2.0 / fan_in)
        layers.append({"w": w, "b": jnp.zeros(fan_out)})
    return layers


def scores(params, x):
    """Score every candidate row. ReLU hidden layers, a linear output."""
    h = x
    for layer in params[:-1]:
        h = jax.nn.relu(h @ layer["w"] + layer["b"])
    out = h @ params[-1]["w"] + params[-1]["b"]
    return out[..., 0]


def loss_fn(params, x, mask, chosen):
    s = scores(params, x)
    s = jnp.where(mask, s, -1e9)
    logp = jax.nn.log_softmax(s, axis=-1)
    return -jnp.take_along_axis(logp, chosen[:, None], axis=-1).mean()


def agreement(params, x, mask, chosen):
    s = jnp.where(mask, scores(params, x), -1e9)
    return float((jnp.argmax(s, axis=-1) == chosen).mean())


def exact_scores(params, x):
    """The forward pass in NumPy float32. On an Ampere GPU JAX's default
    matmul precision is TF32, which is off by a part in a thousand — enough to
    fail the engine's check of its own arithmetic, and the engine's float32 is
    the one the game actually runs."""
    h = x.astype(np.float32)
    for layer in params[:-1]:
        h = np.maximum(h @ np.asarray(layer["w"]) + np.asarray(layer["b"]), 0.0)
    return (h @ np.asarray(params[-1]["w"]) + np.asarray(params[-1]["b"]))[..., 0]


def probe(params, x, mask, mean, std, width_c, n=16):
    rows = x[mask][:: max(1, mask.sum() // n)][:n]
    got = exact_scores(params, (rows - mean) / std)
    return [{"context": r[:width_c].tolist(), "candidate": r[width_c:].tolist(), "score": float(g)}
            for r, g in zip(rows, np.asarray(got))]


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("data", type=pathlib.Path)
    ap.add_argument("--out", type=pathlib.Path, default=pathlib.Path("goal_net.json"))
    ap.add_argument("--hidden", type=int, nargs="+", default=[64, 64])
    ap.add_argument("--epochs", type=int, default=60)
    ap.add_argument("--batch", type=int, default=1024)
    ap.add_argument("--lr", type=float, default=3e-3)
    ap.add_argument("--seed", type=int, default=0)
    args = ap.parse_args()

    schema, games, x, mask, chosen = load(args.data)
    # A tenth of the battles held out, drawn at random: holding out by
    # `game % 10` would hold out one map under one planner, because the
    # dataset assigns both by the game number.
    unique = np.unique(games)
    held = np.random.default_rng(args.seed + 1).choice(unique, size=max(1, len(unique) // 10), replace=False)
    val = np.isin(games, held)
    mean, std = standardisation(x[~val], mask[~val])
    xs = np.where(mask[..., None], (x - mean) / std, 0.0).astype(np.float32)
    print(f"{len(chosen)} decisions, {mask.sum(1).mean():.1f} candidates each, "
          f"{val.sum()} held out by battle; devices {jax.devices()}")

    # What a network has to beat to have learned anything.
    width_c = len(schema["context"])
    worth = width_c + schema["candidate"].index("worth")
    v_first = (chosen[val] == 0).mean()
    v_worth = (np.argmax(np.where(mask[val], x[val, :, worth], -1e9), axis=1) == chosen[val]).mean()
    print(f"baselines on held-out: always-first {v_first:.3f}, highest-worth {v_worth:.3f}")

    key = jax.random.key(args.seed)
    params = init(key, [x.shape[-1], *args.hidden, 1])
    steps = args.epochs * int(np.ceil((~val).sum() / args.batch))
    opt = optax.adamw(optax.cosine_decay_schedule(args.lr, steps), weight_decay=1e-4)
    state = opt.init(params)

    @jax.jit
    def step(params, state, x, mask, chosen):
        loss, grads = jax.value_and_grad(loss_fn)(params, x, mask, chosen)
        updates, state = opt.update(grads, state, params)
        return optax.apply_updates(params, updates), state, loss

    tx, tm, tc = (jnp.asarray(a[~val]) for a in (xs, mask, chosen))
    vx, vm, vc = (jnp.asarray(a[val]) for a in (xs, mask, chosen))
    rng = np.random.default_rng(args.seed)
    start = time.time()
    for epoch in range(args.epochs):
        order = rng.permutation(len(tc))
        for i in range(0, len(order), args.batch):
            idx = order[i : i + args.batch]
            params, state, loss = step(params, state, tx[idx], tm[idx], tc[idx])
        if epoch % 10 == 9 or epoch == args.epochs - 1:
            print(f"epoch {epoch + 1:3d}  loss {float(loss):.4f}  "
                  f"train {agreement(params, tx, tm, tc):.4f}  held-out {agreement(params, vx, vm, vc):.4f}")
    print(f"trained in {time.time() - start:.1f} s")

    net = {
        "kind": "goal_scorer",
        "activation": "relu",
        "context": schema["context"],
        "candidate": schema["candidate"],
        "mean": mean.tolist(),
        "std": std.tolist(),
        "layers": [{"w": np.asarray(l["w"]).tolist(), "b": np.asarray(l["b"]).tolist()} for l in params],
        "trained_on": {"data": str(args.data), "decisions": int(len(chosen)),
                        "held_out_agreement": agreement(params, vx, vm, vc)},
        # Raw inputs and the score JAX gave each, so the engine can check its
        # own forward pass against the trainer's when it loads the network.
        "probe": probe(params, x, mask, mean, std, width_c),
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(net))
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()

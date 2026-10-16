"""Write an untrained commander network (REIMAGINED.md L3): near-uniform
choices over the mission menu, standardised on the decisions of a rollout
played with `--commander uniform --arrays`, in the engine's format with a
probe computed exactly.

    uv run fresh_command.py ../target/rl/uniform.jsonl --out ../target/rl/cmd_fresh.json
"""

import argparse, json, pathlib
import jax, numpy as np
from imitate import exact_scores, init, standardisation

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument("rollouts", type=pathlib.Path, help="a rollout written with --commander uniform --arrays")
ap.add_argument("--out", type=pathlib.Path, required=True)
ap.add_argument("--hidden", type=int, nargs="+", default=[128, 128])
ap.add_argument("--scale", type=float, default=0.01, help="the output layer's weights, so every score starts near zero")
ap.add_argument("--prefer", action="append", default=[], metavar="FEATURE=LOGIT",
                help="start with this much more score on options showing a candidate feature (e.g. free=3): "
                     "a warm start toward a known policy, carried by one hidden unit")
args = ap.parse_args()

beside = lambda ext: pathlib.Path(str(args.rollouts) + ".cmd" + ext)
shape = json.loads(beside(".arrays.json").read_text())
counts = np.fromfile(beside(".counts.u32"), np.uint32).astype(np.int64)
context = np.fromfile(beside(".context.f32"), np.float32).reshape(-1, shape["context"])
candidates = np.fromfile(beside(".candidates.f32"), np.float32).reshape(-1, shape["candidate"])
rows = np.concatenate([np.repeat(context, counts, axis=0), candidates], axis=1)
mean, std = standardisation(rows, np.ones(len(rows), bool))
layers = init(jax.random.key(11), [rows.shape[1], *args.hidden, 1])
layers[-1]["w"] = layers[-1]["w"] * args.scale
# A preference is one hidden unit reading one standardised candidate feature
# through a ReLU, so an option showing the feature scores `logit` more and one
# without it scores nothing extra; the rest of the network starts near zero.
for unit, pref in enumerate(args.prefer):
    name, logit = pref.split("=")
    at = shape["context"] + shape["candidate_names"].index(name)
    on = (1.0 - mean[at]) / std[at]
    first = np.asarray(layers[0]["w"]).copy()
    first[:, unit] = 0.0
    first[at, unit] = 1.0
    layers[0]["w"] = first
    layers[0]["b"] = np.asarray(layers[0]["b"]).copy()
    layers[0]["b"][unit] = 0.0
    # Pass the unit's value straight through the remaining hidden layers.
    for layer in layers[1:-1]:
        w = np.asarray(layer["w"]).copy()
        w[unit, :] = 0.0
        w[:, unit] = 0.0
        w[unit, unit] = 1.0
        layer["w"] = w
        layer["b"] = np.asarray(layer["b"]).copy()
        layer["b"][unit] = 0.0
    last = np.asarray(layers[-1]["w"]).copy()
    last[unit, 0] = float(logit) / on
    layers[-1]["w"] = last
probe_rows = rows[:: max(1, len(rows) // 16)][:16]
got = exact_scores(layers, (probe_rows - mean) / std)
wc = shape["context"]
net = {
    "context": shape["context_names"],
    "candidate": shape["candidate_names"],
    "mean": mean.tolist(),
    "std": std.tolist(),
    "layers": [{"w": np.asarray(l["w"]).tolist(), "b": np.asarray(l["b"]).tolist()} for l in layers],
    "probe": [{"context": r[:wc].tolist(), "candidate": r[wc:].tolist(), "score": float(g)}
              for r, g in zip(probe_rows, got)],
    "trained_on": {"fresh": True, "standardised_on": str(args.rollouts), "decisions": int(len(counts)),
                   "prefer": args.prefer},
}
args.out.parent.mkdir(parents=True, exist_ok=True)
args.out.write_text(json.dumps(net))
print(f"wrote {args.out}: {len(counts)} decisions, {len(rows)} options, {rows.shape[1]} features")

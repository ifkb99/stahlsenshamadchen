"""Write an untrained goal network (near-uniform choices) in the engine's
format: the control for whether the RL loop can learn anything at all."""

import argparse, json, pathlib
import jax, numpy as np
from imitate import exact_scores

ap = argparse.ArgumentParser()
ap.add_argument("like", type=pathlib.Path, help="a trained network to copy names and standardisation from")
ap.add_argument("--out", type=pathlib.Path, required=True)
ap.add_argument("--scale", type=float, default=0.01)
args = ap.parse_args()
net = json.loads(args.like.read_text())
key = jax.random.key(7)
layers = []
for l in net["layers"]:
    w = np.asarray(l["w"], np.float32)
    key, sub = jax.random.split(key)
    layers.append({"w": (np.asarray(jax.random.normal(sub, w.shape)) * args.scale).astype(np.float32),
                   "b": np.zeros(len(l["b"]), np.float32)})
net["layers"] = [{"w": l["w"].tolist(), "b": l["b"].tolist()} for l in layers]
mean, std = np.asarray(net["mean"], np.float32), np.asarray(net["std"], np.float32)
width_c = len(net["context"])
for p in net["probe"]:
    x = (np.concatenate([p["context"], p["candidate"]]).astype(np.float32) - mean) / std
    p["score"] = float(exact_scores(layers, x[None])[0])
net["trained_on"] = {"fresh": True, "scale": args.scale}
args.out.write_text(json.dumps(net))
print("wrote", args.out)

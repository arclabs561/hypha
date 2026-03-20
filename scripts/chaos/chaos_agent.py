#!/usr/bin/env python3
"""
Hypha Chaos Agent: Maps high-level failure intents to netns_chaos.sh runs.
Use this to generate reproducible, nasty scenarios for project scrutiny.
"""
import sys
import argparse
import random

def main():
    parser = argparse.ArgumentParser(description="Chaos Agent for Hypha Scrutiny")
    parser.add_argument("intent", choices=["stall", "flap", "churn", "storm"], 
                        help="High-level failure intent")
    parser.add_argument("--topology", choices=["pair", "line"], default="line")
    parser.add_argument("--transport", choices=["tcp", "quic"], default="quic")
    parser.add_argument("--seed", type=int, default=None)
    
    args = parser.parse_args()
    
    seed = args.seed if args.seed is not None else random.randint(1, 10000)
    
    # Map intent to netns_chaos.sh arguments and duration
    config = {
        "stall": (25, "Massive network stall (80% drop) requiring Spike recovery"),
        "flap": (20, "Rapid link flapping (up/down) every 500ms"),
        "churn": (30, "Process churn: kill/restart relay mid-run"),
        "storm": (40, "Combined stress: high jitter + flapping + churn"),
    }
    
    duration, desc = config[args.intent]
    
    print(f"### Chaos Agent Recommendation: {args.intent.upper()} ###")
    print(f"Invariant to test: {desc}")
    print(f"Command:\n  scripts/chaos/netns_chaos.sh {args.topology} {args.transport} {seed} {duration}")
    print(f"\nTo replay this exact run, use seed: {seed}")

if __name__ == "__main__":
    main()

# Network lab worksheet

For each run, record:
- Date/time and access network.
- Relay region/provider and endpoint.
- Tunnel enabled/disabled.
- Median, p95 and p99 latency to a controlled test endpoint.
- Jitter calculation method.
- Probe count and loss rate.
- WireGuard handshake age and transfer counters (`wg show` on relay).
- Any route or firewall changes.

Change one variable at a time. Compare multiple runs; do not infer improvement from a single ping. A tunnel ping to `10.66.0.1` measures the client-to-relay path, not the complete path to a game server.

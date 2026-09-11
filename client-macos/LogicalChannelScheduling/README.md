# LogicalChannelScheduling

Platform-neutral weighted scheduler for SRUI logical channels (§18.2, §19.2).

This is a nested Swift package so Linux CI can compile and test the algorithm without
building `TransportSSH`, AppKit, or the rest of `client-macos`. `TransportSSH` depends on
this package; do not copy the cycle or selector into the transport target.

The 24-slot cycle and `maxServiceGap` values are generated from
`protocol/logical-channel-policy.yaml` into `LogicalChannelPolicy.generated.swift`.

```bash
swift test --package-path client-macos/LogicalChannelScheduling
```

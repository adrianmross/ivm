# IVM

Small Istio version and profile manager. Profiles pin an Istio client version,
Kubernetes context, and the `istioctl install` options for one environment.

## Quick start

```sh
ivm profile set fabric \
  --context fabric-ivm \
  --istio-version 1.22.1 \
  --set profile=default \
  --set values.pilot.env.ENABLE_TLS_ON_SIDECAR_INGRESS=true \
  --set components.cni.enabled=true \
  --set values.cni.repair.deletePods=true

ivm profile set besu \
  --context besu-ivm \
  --istio-version 1.28.3 \
  --set profile=default \
  --set values.pilot.env.ENABLE_TLS_ON_SIDECAR_INGRESS=true \
  --set components.cni.enabled=true \
  --set values.cni.repair.deletePods=true

ivm profile use fabric
ivm install # download/cache the pinned istioctl
ivm apply   # run istioctl install against the pinned context
ivm unapply # run istioctl uninstall --purge -y
ivm apply -d # same as unapply
ivm status
```

Switching profiles does not modify `PATH` or a Kubernetes cluster. `ivm apply`
uses the selected profile's exact `istioctl` binary and context.

To remove the selected mesh from Kubernetes:

```sh
ivm unapply
# or: ivm apply -d
```

To remove only the locally cached client:

```sh
ivm uninstall
```

The configuration lives at `~/.config/ivm/config.toml`. Set `IVM_CONFIG` for a
different file, which is useful for tests.

## Development

```sh
cargo test
cargo run -- profile list
```

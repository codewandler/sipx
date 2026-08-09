# Design: resolving cluster-internal SIP names

**Status:** proposed · **Pillar:** Core · **Epic:** `cluster-names` · **Stories:** _unallocated_

## Why

An operator running SIP inside Kubernetes knows their endpoint by its service name —
`echo.voice.svc.cluster.local` — and would like to dial it. Today they must first find a pod
address by hand, dial that, and then discover that media does not flow anyway. Every step of that is
mechanical, and every step is somewhere sipx could have helped.

The prize is not "sipx speaks Kubernetes". It is that the name an operator already has becomes a
name sipx can dial, without the operator learning which of the four things that could be wrong is
wrong this time.

## Ordinary DNS first, always

**RFC 3263 resolution runs unchanged and runs first.** NAPTR, then SRV, then A/AAAA. Only when that
yields nothing does anything below happen. This is not deference to convention; three things break
if the order is reversed:

- A cluster name that *is* published in ordinary DNS — split-horizon resolvers, a stub zone, a VPN
  that serves the cluster's suffix — must resolve the way the operator's own infrastructure says,
  not the way sipx guesses. An operator who arranged for that name to resolve has already answered
  the question.
- The fallback reads kubeconfig and may reach the API server. Doing that speculatively, before the
  cheap answer has been tried, turns every dial into a credential read.
- RFC 3263 selects a *transport* as well as an address. A path that skipped it would silently
  demote a `_sips._tcp` service to plain UDP.

So the fallback is precisely: **ordinary resolution returned no address, and the name is a cluster
name.** Never in preference to DNS, and never as an override.

## Recognising a cluster name

The cluster suffix is a cluster's own decision (`--cluster-domain`), not a constant, and
`cluster.local` is only the common default. sipx should treat the suffix as configuration with that
default, and require the shape `<service>.<namespace>.svc.<suffix>` — the four-label form that
carries a namespace. The two-label `<service>.<namespace>` form is deliberately *not* recognised: it
is indistinguishable from an ordinary short name, and guessing there would hijack names that have
nothing to do with any cluster.

## What the fallback actually does

The instinct is to ask the cluster's DNS. That does not work from outside, and the reason it does
not work is the reason for the whole design:

**CoreDNS is reached at a `ClusterIP`, and a `ClusterIP` is not an address.** It is a rule in every
node's packet filter. A VPN that carries the pod and node networks still cannot reach one, because
there is nothing to reach — no interface holds it. Pointing a resolver at it fails in a way that
looks like a timeout and is actually a category error.

So the fallback does not use DNS. It uses the **Kubernetes API**, which is the one endpoint an
operator's kubeconfig already reaches:

1. Read the kubeconfig — `$KUBECONFIG`, else `~/.kube/config` — and take the **current context**.
2. From that context take the **namespace**, defaulting to `default`. A namespace in the name being
   dialled overrides it; a name is more specific than an ambient setting.
3. `GET /apis/discovery.k8s.io/v1/namespaces/<namespace>/endpointslices` filtered to the service,
   which yields **pod addresses and their ports** — the routable ones, the thing a `ClusterIP`
   stands in front of.
4. Offer them to the existing target selection as an ordered candidate list, so failover across
   endpoints is the failover sipx already has and not a second mechanism.

EndpointSlices rather than the `Endpoints` resource: it is the current API, it carries readiness per
address, and an endpoint that is not ready is one this dial should not choose.

## Where this stops, and why that matters more than the resolution

**Resolving the signalling address does not make the call work, and an operator who is not told so
will conclude sipx is broken.** Two failures sit behind it, both observed:

- **Media is negotiated, not resolved.** The address RTP flows to comes from the far end's SDP, and
  a SIP endpoint in a cluster commonly answers with an *external* address, because that is what it
  was configured to advertise for the internet's benefit. Signalling then goes over the tunnel while
  media goes over the default route — two paths, two source addresses — and the leg bound to the
  tunnel address cannot reach the media address at all. Nothing about resolving the name changes
  this. It is why `softphone`'s `--advertise` exists, and why the diagnostic distinguishes "answered"
  from "audio arrived".
- **Private ranges collide.** A cluster's service CIDR is a private range, and so is a VPN's. When
  they overlap, an address advertised from outside names something *inside* the cluster, and the far
  end sends media to its own network in perfect good faith. This is not exotic: default service
  CIDRs and default VPN ranges are drawn from the same small pool.

So the deliverable is not only resolution. It is resolution **plus** a dial-time check that the
address sipx is about to advertise is not inside a range the far end will read as its own, and a
failure message that names which of the two paths did not complete.

## What this does not do

No kubeconfig writing, no context switching, no `port-forward`, no in-cluster credential discovery
beyond reading the file kubectl already reads, and no dependency on a Kubernetes client library in
the core: the API call is one HTTPS GET against a documented resource, and the alternative is a
dependency graph larger than sipx.

## Acceptance sketch

- A name that resolves in ordinary DNS never reaches the fallback — proven by a test whose fake
  resolver answers and whose fake API server fails the test if called.
- A four-label cluster name with no DNS answer produces ordered pod candidates from a fixture
  EndpointSlice, with unready addresses excluded.
- A two-label short name never triggers the fallback.
- Dialling with an advertised address inside the far end's own service CIDR is refused at dial time,
  with a message naming the collision rather than timing out on media.

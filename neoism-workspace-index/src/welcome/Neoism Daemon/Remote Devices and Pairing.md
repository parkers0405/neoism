# Remote Devices and Pairing

Pairing gives a remote Neoism surface a durable device identity and an explicit set of permissions on a daemon host. It is authorization, not network discovery: the device must also be able to reach the host.

## Pairing flow

1. On the host, start the pairing action and request the permissions the device should have.
2. Neoism creates an eight-character, uppercase pairing code designed to avoid ambiguous characters.
3. Enter that code on the remote device together with a device label.
4. The host's approval policy decides which requested permissions are granted.
5. On approval, the remote device receives a device ID and a secret device token for later connections.

A code is single-use, expires after 60 seconds, and exists only in the daemon's memory. Restarting the daemon invalidates outstanding codes. A used, expired, or mistyped code must be replaced with a newly generated code.

Depending on the host's approval configuration, a claim can be granted immediately, left pending, or rejected. The current daemon only grants immediately when host auto-approval is enabled; otherwise it returns pending and does not issue a usable token. Pairing is therefore not complete until the host is configured to grant the request.

## Authentication after pairing

The secret device token is presented when opening the daemon connection and again in the protocol handshake. The daemon verifies the token against its paired-device registry and applies the permissions recorded for that device.

Tokens, unlike short-lived pairing codes, are persisted by the host. The daemon stores a verifier rather than relying on the raw token as a device record. Treat the token as a password: anyone holding it can act with that device's granted permissions until the device is revoked.

Revoking a device removes its authorization for future authenticated requests and connections. Revocation requires device-management permission. If a device is lost or a token may have leaked, revoke it on the host and pair the device again if needed.

## Permissions

Permissions are enforced per operation. The protocol distinguishes capabilities including reading and writing files, creating and using PTYs, using agents, reading or writing the clipboard, and managing devices. Pairing does not inherently grant every capability; the final grant comes from the host's approval decision.

A device can therefore connect successfully but still be denied a particular action. That is different from an invalid token or unreachable host.

## Reachability and remote networks

Neoism can advertise and discover daemon hosts on a configured private network and can show remote workspace trees. Discovery supplies a host address; it does not bypass pairing or permissions. If the host is offline, its private-network address changed, or the viewing device is not on a network route that can reach it, a valid pairing token alone cannot establish a connection.

Files, shells, and workspace services continue to run on the selected host. The remote device is a client of that daemon rather than a second owner of the host filesystem.
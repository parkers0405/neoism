# Connect Another Device

Neoism can connect a desktop, browser, phone, or another laptop to a workspace host. The host runs `neoism-workspace-daemon`, which owns the workspace's PTYs, layout state, pairing credentials, and remote sessions. A prebuilt Neoism installation manages its local daemon for you.

## Decide which machine hosts the workspace

The host is the machine that has the project directory and runs its terminals. A connected device is a client: it displays and controls the host's workspace rather than copying the project into a second unrelated workspace.

For access between your own machines, put them on the same private network or tailnet. Tailscale is the documented route for reaching a host without exposing its workspace service to the public internet.

## Add an existing host

On the device that will connect:

1. Open the command palette.
2. Choose **Servers** and then **Add server**.
3. Stay on the **Join server** tab.
4. Enter the server address supplied by the host, optionally give it a recognizable name, and enter its password if one is required.
5. Choose **Join server**.

The address field accepts the workspace session endpoint; the UI's example is `ws://192.168.1.20:7981/session`. Use the actual address and port shown or configured on your host rather than assuming the example values.

Saved servers appear in the server list. The list also supports editing or forgetting a saved server. Forgetting removes the saved address and local credential; it does not change or stop the remote host.

## Host a project from Neoism

The **Add server** form also has a **Create server** tab. Choose a **Project folder**, optionally set a server name and password, then select **Create & join**. This creates a hosted workspace for that folder on the current machine and joins it.

When authentication is enabled on a daemon, clients must provide the matching credential. Neoism stores saved credentials with the server entry. Do not publish an unauthenticated workspace endpoint on the open internet.

## What follows you

Remote clients connect to daemon-owned workspace state, including terminal sessions and layout. Presence identifies participants using `presence.display-name`; cursor styling can also be shared. Tabs remain per-user, while collaborative document state is synchronized through Neoism's workspace protocols.

Set a recognizable presence name in `config.json`:

```jsonc
{
  "presence": {
    "display-name": "Parker",
    "cursor-style": "rainbow",
  },
}
```

If a connection fails, first verify that the host is running, the address includes the correct scheme, port, and `/session` path, both devices can reach each other, and the password matches the host configuration.

Next: [[06 Configure Neoism|Configure Neoism]].
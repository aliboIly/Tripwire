# Security Policy

Tripwire's local server is privileged. It can read and change the open Studio place and, when you give it an Open Cloud key, read and overwrite live DataStores, publish places, upload assets, and message running servers. A bug that lets the wrong input reach those paths matters. Please report it privately.

## Reporting a vulnerability

Do not open a public issue, pull request, or discussion for a security problem.

Use one of these private channels:

- Open a private advisory at https://github.com/aliboIly/Tripwire/security/advisories/new (the Security tab, then Report a vulnerability).
- Or email ali@alibolly.com with "Tripwire security" in the subject.

Please include:

- The version you are on (server version, and plugin version if Studio is involved).
- Which path is affected: the Studio bridge, the Open Cloud client, or the security reviewer.
- Steps to reproduce, and what an attacker could do with it.
- Any logs, with secrets removed.

## What to expect

This is a small project maintained in spare time. You should get an acknowledgement within about a week. From there we will confirm the issue, agree on a fix and a disclosure timeline, and credit you in the release notes if you want the credit. Please give a reasonable window to ship a fix before any public disclosure.

## Scope

In scope:

- The server leaking, logging, or mishandling the Open Cloud key or any token.
- The local bridge accepting commands it should not, or from a peer it should not.
- The injected runner or the plugin doing something the calling tool did not ask for.

Out of scope:

- What an Open Cloud key can do once you grant it scopes. That power is yours to manage. Grant only the scopes you use, restrict the key to your own IP, and revoke it if it leaks. The README says this too.
- Issues in Roblox Studio, Open Cloud, or roblox-ts themselves. Report those to Roblox or the relevant project.

## Supported versions

Fixes land on the latest release. There is no long-term support branch yet, so please upgrade to the newest version before reporting.

## How Tripwire handles your key

The Open Cloud key is a real credential. Tripwire reads it from an environment variable or a gitignored `.env`, never logs it, and never commits it. If you find a path where a key or token could leak into logs, output, or the wire protocol, treat it as a security issue and report it through the private channels above.

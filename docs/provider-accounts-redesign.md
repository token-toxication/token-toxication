# Provider Accounts admin redesign

**Status:** Implemented
**Date:** 2026-10-10
**Scope:** Admin UI information architecture and account lifecycle messaging. The first implementation does not require a server API change.

## Problem

Provider Accounts currently presents upstream accounts as generic connection rows. The collection table shows provider, protocol, endpoint, routes, load, and health, but hides the authentication mode. The account detail page also omits authentication mode from its Connection card.

This makes the two Codex credential lifecycles look identical. A device-login account is managed by the relay, while a `codex-manual-refresh` account contains a manually entered refresh token and is deprecated. The deprecation warning currently appears only after opening the edit form, so an operator cannot identify accounts that need attention from the account list or detail page.

The add/edit sheet also mixes account identity, authentication, endpoint, and scheduling controls into one undifferentiated form.

## Goals

- Make authentication mode a first-class account property in the collection and detail views.
- Make deprecated Codex accounts visible before an operator edits them.
- Explain how each credential is obtained and maintained without exposing secrets.
- Keep provider routes, health, and concurrency information available for operational work.
- Keep the current device-login, manual-refresh compatibility, Antigravity OAuth, and API-key behavior unchanged.

## Non-goals

- Change the provider-account API or credential envelopes.
- Remove `codex-manual-refresh` in this redesign.
- Add secret inspection, token export, or token rotation controls to the browser.
- Rework model catalog or provider-route pages.

## Collection page

Keep the page as a single Provider Accounts workspace with four layers. The
redesign is additive: every piece of information currently visible in the
desktop table remains visible, while authentication becomes a first-class
field.

The collection content should use a readable max width instead of stretching
to the full space beside the navigation rail. The account list is a dense
stack of rows with a stable two-column structure, not a nine-column grid.

1. **Header**
   - Title, short description, and **Add account** action.
2. **Deprecation notice**
   - Render only when one or more accounts use a legacy Codex mode.
   - Explain that manual refresh tokens are deprecated and that new accounts should use Codex device login.
   - Highlight the affected rows. The notice must be visible without opening an edit form.
3. **Responsive account list**
   - Replace the wide, horizontally-scrolling table with dense account rows.
   - The left side of each row shows the account name, provider, and
     authentication badges.
   - The middle side shows the truncated base URL with the wire protocol below
     it, then routes and running/queued capacity as compact metadata.
   - The right side shows active/paused plus health status and the existing
     account actions. Secondary actions can move into an overflow menu only if
     their labels and tooltips remain discoverable.
   - Base URLs may truncate visually, but must expose the full value through a
     tooltip or detail page link.
   - The account name links to the account detail page, while quota, reconnect,
     toggle, and edit actions remain direct.
4. **Small-screen layout**
   - Keep the same dense row as a vertical card: account/provider, auth badges,
     endpoint/protocol, routes, running/queued traffic, status, and actions.
   - Never omit authentication mode or any of the current operational fields on
     narrow screens.

A wide-screen row should read approximately as follows:

```text
Codex Pro                         Healthy       [quota] [edit] [more]
codex-subscription  [Device login] [Managed]
https://chatgpt.com/backend-api · Responses    4 routes · 0/4 running
```

The first line establishes identity and health. The second line makes the
credential lifecycle explicit. The third line preserves endpoint, protocol,
route count, and capacity without forcing the page into a horizontally wide
table. On a small screen the same three lines stack naturally inside the row.

The auth label should be derived from the existing provider preset metadata where possible, with explicit fallbacks for stored modes:

| Stored mode | Display label | Lifecycle state |
| --- | --- | --- |
| `codex-device-oauth` | Codex device login | Managed |
| `codex-manual-refresh` or `codex-oauth` | Manual refresh token | Deprecated |
| `antigravity-oauth` | Antigravity OAuth | Managed |
| `x-api-key` | API key | Manual |
| `x-goog-api-key` | Google API key | Manual |
| `bearer` | Bearer token | Manual |

Unknown modes should remain visible as their raw value rather than being hidden or shown as an empty field.

## Account detail page

Use a two-column layout on wide screens and a single vertical layout on small screens.

### Identity card

Show provider, authentication mode, wire protocol, and base URL. The auth mode is the first item and uses the same label and lifecycle badge as the collection page.

### Credential lifecycle card

Show a short, mode-specific explanation:

- **Codex device login:** sign-in was completed through the official device flow; access and refresh credentials are managed by the relay; no browser secret is editable.
- **Manual refresh token:** this is a legacy credential path; new accounts should use device login. Keep the credential itself hidden.
- **Other modes:** describe whether the relay sends an API key or bearer token.

For legacy Codex accounts, keep a destructive-style warning visible in the detail page. Include a **Create a new Codex device-login account** action that opens the add-account flow with the Codex device-login preset; route bindings remain an explicit operator action.

### Runtime information

Keep active/schedulable state, health, max running requests, current
running/queued load, and creation time in the Connection card.

### Provider Model Routes card

Keep the existing linked-route table and empty state. The card should remain operationally independent from credential lifecycle messaging.

## Add and edit flow

Organize the sheet into three labeled sections:

1. **Account**
   - Preset, name, provider, base URL, and protocol.
2. **Authentication**
   - Auth mode as a set of descriptive options rather than an unexplained raw-value select.
   - Mode-specific help immediately below the selected option.
   - Credential input only when that mode requires manual input.
3. **Routing and limits**
   - Upstream path, max running requests, and schedulable toggle.

New account creation continues to hide the deprecated manual Codex mode. Editing a legacy account continues to allow the existing credential to be retained, but the deprecation alert must appear at the top of the Authentication section and in the account detail view.

The device-login and Antigravity actions remain explicit sign-in actions. Their forms should explain that the browser opens a verification flow and that no token needs to be pasted.

## Data and implementation boundary

The existing `ProviderAccount.authMode` field is sufficient for collection and detail display. `ProviderPreset` already supplies labels and credential guidance. The UI should add shared presentation helpers instead of duplicating raw mode strings across table, detail, and form components.

The implementation remains frontend-only:

- Shared auth-mode display and lifecycle helpers are used by the collection,
  detail, and form views.
- The collection uses a max-width responsive account-card layout with a legacy
  notice, while retaining the existing operational fields and actions.
- The detail view has explicit authentication and credential-lifecycle cards,
  including a direct migration CTA for legacy Codex accounts.
- The add/edit sheet groups account, authentication, and routing controls and
  keeps legacy manual refresh accounts editable for compatibility.
- English and Chinese catalog entries plus focused rendering tests cover the
  managed and deprecated modes.

## Acceptance criteria

- Every wide account row and mobile card shows the account's auth mode.
- The account detail page shows auth mode and lifecycle state without opening Edit.
- Any legacy Codex account causes a visible deprecation notice on the collection page.
- Editing a legacy account still preserves the current credential when left blank.
- Device-login accounts never expose access or refresh tokens in the UI.
- Existing quota, model inspection, route, toggle, reconnect, and delete actions continue to work.
- The UI has focused tests for device-login, legacy Codex, and a normal API-key account.

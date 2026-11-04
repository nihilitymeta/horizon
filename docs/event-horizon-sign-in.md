# Setting Up Microsoft Sign-in

Event Horizon's launcher starts the game only for a signed-in Microsoft account that owns Minecraft: Java Edition.
Signing in needs an app registration of Event Horizon's own with Microsoft, approved by Mojang. This page walks
through it. You do it once; nothing else in Event Horizon changes.

Use your own registration, never another application's client ID, Horizon's included. Its consent page would show
Horizon's name to your developers, and Horizon's owner could turn it off for everyone at once.

## What you need

- A personal Microsoft account. It can be the one you play with.
- About 20 minutes for the Microsoft part.
- Patience for Mojang's approval, which takes days to weeks.

## 1. Register the app

1. Open the [Azure portal](https://portal.azure.com) and sign in with your Microsoft account. If it asks you to set
   up a free Azure account, do so; app registrations cost nothing.
2. Search for **App registrations**, open it, and select **New registration**.
3. Fill in the form:

   | Field | Value |
   |---|---|
   | Name | `Event Horizon`. Your developers see this when they sign in |
   | Supported account types | **Personal Microsoft accounts only** |
   | Redirect URI | leave it empty |

4. Select **Register**.
5. On the app's page, open **Authentication**. Under **Advanced settings**, set **Allow public client flows** to
   **Yes**, and save. This allows the sign-in code the launcher shows. Event Horizon has no client secret, so don't
   create one.
6. Leave **API permissions** as it is. The launcher asks for what it needs when someone signs in.
7. Go back to **Overview** and copy the **Application (client) ID**. It looks like
   `12345678-abcd-ef01-2345-6789abcdef01`.

The client ID is not a secret: anyone can see it in the launcher. Keep it handy for the next steps.

## 2. Ask Mojang to approve it

Until Mojang approves the app, sign-in fails at the Minecraft step with "The Minecraft services refused this
launcher: its app is not approved by Mojang yet."

1. Open Mojang's [app review form](https://aka.ms/mce-reviewappid) (Minecraft Java "App ID review").
2. Give it:

   | Field | Value |
   |---|---|
   | Client ID | the ID from step 1 |
   | App name | Event Horizon |
   | Project | `https://github.com/whos-carmen/horizon` |
   | Description | A fork of Horizon, an open-source Minecraft: Java Edition client written in Rust, for developing one mod. It signs players in to play and to join online-mode servers. |

3. Wait for Mojang's answer by email.

You can do step 3 below while you wait; sign-in starts to work the day Mojang approves.

## 3. Give the ID to Event Horizon

**For releases.** Store the ID as a variable of the repository, so the release workflow builds it into the launcher:

```sh
gh variable set HORIZON_MSA_CLIENT_ID -R whos-carmen/horizon --body <client id>
```

The next release's launcher can sign in. A release made before you set it can't, and the workflow warns about it.

**For launchers you build yourself.** Set it in your shell profile (`~/.zshrc` on macOS), so every build and every
run gets it:

```sh
export HORIZON_MSA_CLIENT_ID=<client id>
```

The launcher reads it when it is built, and again when it runs, so an already built launcher works too once you
start it from a shell that has it.

## Until then

A launcher with no client ID can still play with an account that signed in elsewhere, as long as it shares the
launcher's data folder: sign in once with Horizon's own launcher, then start Event Horizon's.

That account has to renew its Minecraft session about once a day, and renewing needs the client ID the account
signed in with. When the session runs out, the launcher says "This build of the launcher has no Microsoft sign-in";
sign in with Horizon's launcher again. Once your own app is approved, sign in with Event Horizon's launcher, and the
account renews itself from then on.

## If something goes wrong

| What the launcher says | What it means |
|---|---|
| This build of the launcher has no Microsoft sign-in | the launcher has no client ID: step 3 |
| The Minecraft services refused this launcher: its app is not approved by Mojang yet | Mojang hasn't approved the app: step 2 |
| This account doesn't own Minecraft: Java Edition | the account signed in, but has no Java Edition |

The whole sign-in flow, for reference: [microsoft-account.md](microsoft-account.md).

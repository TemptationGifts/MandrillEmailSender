# Mandrill Sender

A small Windows desktop email sender written in Rust, with multiple recipients,
attachments, rich formatting, embedded images and local message history.
Configure your own Mandrill account through the app's Admin settings.

1. On first use, open **Admin**, enter your organisation's Mandrill API key and
   click **Load domains from API key**. This loads domains Mandrill reports as
   verified with valid signing. Edit or reorder the list if needed and click
   **Save settings**. You can also enter domains manually, one per line.
2. Type any valid name before the **@** in **From**, then choose a configured
   domain. Sender name and Reply to are optional. There are no built-in API keys,
   sending addresses or organisation-specific domain restrictions.
3. Enter recipients in **To**, **CC** and/or **BCC**. Each field accepts multiple
   email addresses separated by commas, semicolons or line breaks. Use addresses
   only, without display names. To and CC are visible; BCC is hidden.
4. Enter a subject and message. Select text and use **B**, **I**, **U**, size,
   colour, **Bullets** or **Link** to format it. Ctrl+B/I/U and Undo/Redo work too.
   Formatting is visible while writing; pasted text is inserted as plain text.
5. Use **Insert image** for PNG, JPEG or GIF pictures in the body. They appear as
   image labels while writing; **Preview** displays the pictures in position.
   Set an image's display width or remove it below the editor. Images are embedded
   in the email itself and do not require image hosting. Use **Add files** for
   ordinary attachments. Both HTML and a plain-text fallback are sent.
6. Click **Send email**. Results show each recipient's sent, queued or failed
   status. Queued messages have not yet been sent; neither status proves inbox
   delivery. Use **New email** to compose another message.

**Check connection** tests the currently saved key without sending an email.
In **Admin**, **Test key** tests a proposed replacement before it is saved.
Changing the key clears the proposed domain list so it can be loaded or entered
for that account. Closing or cancelling Admin discards unsaved changes.

Admin is a settings panel, not a password-protected role or access-control system.
Keys and domain lists are per Windows user; changing them does not update other
people using the shared executable. Domain loading is read-only: it does not add,
verify or delete domains in Mandrill. The app does not modify IP allowlists.

The app sends only when **Send email** is clicked. It does not automatically retry.
After a timeout, partial result or other uncertain outcome, check Mandrill Activity
before using **Edit & send again**, and remove recipients already accepted.

Attachments and embedded images share a 6 MB total limit, leaving space for
base64 encoding and both bodies under Mandrill's 10 MB API JSON limit. The exact
encoded request size is also checked. The body limit is 1 MB.
If an attachment is moved, removed or unreadable, the email is not submitted.

Sender defaults are saved in `%LOCALAPPDATA%\MandrillSender\settings.json`.
The API key is saved in `%LOCALAPPDATA%\MandrillSender\admin.json`, encrypted with
Windows DPAPI for the current user. Domains are stored in the same file. A fresh
profile has no configured credentials; sending is disabled until configured.
Copied encrypted settings may not unlock on another PC or Windows account; use
Admin to enter the organisation's key there. Keys are not stored in message History.
The local name, selected domain, sender name and Reply-To are remembered when
you send an email or close the app normally. Existing saved values are preserved.
**History** records sent emails and send attempts in
`%LOCALAPPDATA%\MandrillSender\history`. It saves sender details, all recipients,
subject, formatted body, copies of attachments and embedded images, and the initial API result.
Reopen any entry to read it, save an attachment, or **Copy into a new email**.
Nothing is resent automatically. History survives closing or moving the executable
on this PC, and has no automatic expiry. Copying only the executable to another PC
does not transfer history. Back up the `MandrillSender` folder to preserve it.

History shows the status returned at send time; it does not update queued messages
or fetch later bounces. Current delivery activity is available in Mandrill.
There is no inbox. Unsent drafts are not saved.

## Source and building

Requires a Rust toolchain and a Windows linker. The delivered executable runs
without a Rust installation, Python, Node or a local web server.

No API key is required to build, and no key is embedded in the executable.
Source can be shared between organisations; configure credentials through Admin.
Runtime settings and History live outside the repository. The ignore rules also
exclude common local settings files, old key files and compiled outputs.

```powershell
cargo test --locked
cargo build --release --locked
```

With Cargo on your PATH, `./build.ps1` builds and copies the executable to
`dist\Mandrill Sender.exe`. Use `./build.ps1 -Test` to run the tests, or
`-OutputDirectory dist\release` to choose a different output folder.
Lato's font files and OFL license are bundled into the executable; the license
is available through **About**.

Tests use synthetic addresses and attachment files. They do not send emails.
`Mandrill Sender.exe --check-connection` performs a read-only authentication check
and returns exit code 0 on success or 1 on failure.

References: [Mandrill send API](https://mailchimp.com/developer/transactional/api/messages/send/),
[sender domain API](https://mailchimp.com/developer/transactional/api/senders/domains),
[attachment limits](https://mailchimp.com/developer/transactional/docs/outbound-email/).

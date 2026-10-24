# Security

The current supported release is 0.1.0.

Please use the repository's **Security → Report a vulnerability** feature for
potential credential exposure or other security issues. Do not post tokens,
authentication files or personal provider responses in a public issue.

Secrets added in the application are stored in Windows Credential Manager.
Existing CLI authentication remains in the provider's own local profile.
The renderer receives usage readings, not authentication tokens.

The portable executable is unsigned. Release checksums identify the published
files; they are not a code-signing certificate. Do not disable Windows security
features to run this application.

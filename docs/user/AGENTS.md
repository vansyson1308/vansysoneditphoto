# User docs: rules for agents and contributors

Everything in this folder (except this file, CLAUDE.md and README.md) is the MagicPic user
guide. It is read on the project page (the app's **User guide** link opens this folder); it is
not published on a website. Other pages and the app link here by file path, so renaming a file
breaks links: add the old path to `redirect_from` when you rename.

These pages came with the fork from PhotoCraft. Version numbers below 0.23 in them, and links to
`storytold/photocraft` issues, refer to that project's history.

## Who reads this

Artists using MagicPic, people answering them, and AI assistants looking things up for
either. Write for an artist, not a developer.

## Page rules

- One question or problem per page. Lead with the answer or the fix in the first two lines.
- Plain words, short sentences, numbered steps for anything the reader does.
- Start the body at `##`. The page title comes from frontmatter.
- Link to other pages instead of repeating them. Use relative links to the `.md` file:
  `[Install MagicPic](../getting-started/install.md)`.
- No marketing copy, no "simply", no claims you have not checked in the code or a release.
- Call the product "MagicPic", as the app does.
- Name UI labels exactly as the app shows them, in **bold**. Menu paths read `File › Open…`.
- Troubleshooting pages: `## Symptoms`, `## Fix`, `## Why this happens` (optional),
  `## Still stuck?` (link the issue tracker).

## Frontmatter

    ---
    title: Export an image as PNG or JPEG
    description: One sentence for search results, link previews and AI summaries.
    order: 10                 # optional, sorts within the folder (lower first)
    verified: 0.23.0          # optional, app version you last checked this page against
    redirect_from: [old/path] # optional, old paths (relative to this folder)
    ---

Only `key: value` and `key: [a, b]` lines work. Quote values that start with a quote or contain ` #`.

Set `verified` to the version you checked the page against (`[workspace.package] version` in the root `Cargo.toml`). There are no MagicPic releases yet, so pages describe `main`.

## Folders

- `getting-started/` install and first run
- `guides/` how to do one thing
- `troubleshooting/` symptom → fix
- `faq/` short answers
- `reference/` lists and settings

A folder's `index.md` names the folder in the sidebar. Files or folders starting with `_` are
drafts and are not published.

## Check before you push

Open the page on the project site or in a Markdown preview and follow every link. The checker
the upstream project used lives in a repository of theirs and is not part of MagicPic.

## Support PRs

Pages written to answer a user question come in as PRs titled `docs(support): <page title>`.
Review them like code: check every claim against the source.

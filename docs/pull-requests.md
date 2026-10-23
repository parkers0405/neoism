# Pull requests

Neoism integrates GitHub pull requests into the Git sidebar and the existing review editor. Local Changes remains the place to edit, stage, restore, and commit working files. Pull request tabs show immutable hosted comparisons and never acquire an editable source buffer.

## Setup

Install the GitHub CLI (`gh`) and run `gh auth login` on the machine that owns the repository. For a joined or remote workspace, authenticate on that host, not on the viewing device. Neoism does not reuse Copilot credentials, copy tokens to clients, or open a browser on the remote host.

The initial provider supports explicit `github.com` remotes. GitLab and GitHub Enterprise are not supported. Open the repository root as the declared workspace: nested directory declarations are rejected rather than silently granting access to an enclosing repository.

## Everyday workflow

The Git sidebar shows a compact PR row only when the current branch has an actual linked pull request. No PR means no row, placeholder, loading strip, or discovery-error banner. Select the linked PR's number/title to open its review tab. Use Create pull request in the Git actions menu to start a draft/ready PR, or Open pull request to enter another PR number without checking it out. Creation first refreshes branch/head metadata; it never silently publishes commits or saves edited documents. Commit and push the branch before creating a PR. Automatic discovery failures stay out of the sidebar; errors from an explicitly requested operation appear as notifications.

A PR tab has Changes, Conversation, and Checks views. Changes reuses the multi-file unified/split diff renderer. Hover a source line's comment affordance to write an inline comment, queue comments, and submit them together with Comment, Approve, or Request changes. Threads can be replied to and resolved. Browser links open on the viewing device.

Comments are anchored to the source file, side, line, and expected PR head, not to wrapped display rows. Refresh preserves drafts. If the head changes, stale drafts cannot be submitted automatically; discard and recreate their anchors against the new comparison. Closing a tab with an unsent draft requires an explicit discard decision or completing the draft first.

## Pull Requests inbox

Open the Git actions menu and choose Pull Requests to browse the connected repository. The inbox has text and label search, sort and state filters, and groups PRs by your relationship to them. Rows use real repository, author, update, check, and change-count metadata. Results are paginated; this is not an account-wide or multi-repository feed.

Select a row to keep the list beside the selected PR's detail. Summary shows the description, labels, and reviewers; Timeline shows conversation and reviews; Code uses the existing immutable comparison and review controls. The detail title, actions, and Summary / Timeline / Code navigation scroll away with the document so Code can use the full detail height. Scrolling back up returns the header after reaching the top of the code or conversation. Pending reads use skeleton placeholders instead of loading text; completed empty results and errors remain explicit. Narrow windows show one pane at a time with a way back to the list. Switching rows preserves cached unsent review drafts, and closing the inbox uses the same discard safeguards as individual PR tabs.

## Checkout

Inspecting a PR does not check it out. Check out and New worktree are explicit operations and are unavailable while review drafts remain unsent. Neoism never implicitly stashes, forces a checkout, or overwrites an existing destination.

Changing the current checkout requires an attached authoritative daemon. The host checks Git status and every open repository document under the document operation gate, then reconciles clean documents before acknowledging success. Unsaved collaborative edits refuse the checkout. New worktrees are generated under the declared repository's managed `.neoism-pr-worktrees` directory, not at a caller-supplied path outside the workspace. The managed directory is excluded locally through Git's common `info/exclude`; tracked ignore files are not edited.

Generated checkout branches retain the original PR's source repository and branch. Their dedicated remote and explicit non-force push refspec send subsequent pushes to that source, including fork PRs. They are not published as unrelated new branches. Discovery revalidates this association rather than inferring the PR from the generated local branch name.

Native adopted-host workspaces that are not remote-joined cannot navigate directly to a new host-side worktree yet. Neoism reports its host path; select that directory through the host workspace picker instead. It never interprets a foreign host path as a local filesystem path.

## Safety and limits

Hosted mutations require the host's Git-write permission. Requests are not replayed after reconnection. If a submission loses its acknowledgement, verify its state on GitHub before retrying: a network failure cannot prove that GitHub did not apply a mutation.

Comparisons and collections are bounded. Missing binary or oversized patches and capped conversation/check results are marked as partial; use Open in browser for complete detail. Source-head and base revisions are rechecked while loading a comparison. PR errors do not disable ordinary local Git operations.

The current UI supports single-line inline comments and plain-text conversation rendering. Multiline comment ranges, individual queued-comment editing, and native merge controls are not exposed yet; use GitHub for those operations.

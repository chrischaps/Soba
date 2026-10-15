"""Claims an issue so that agents on other machines leave it alone.

    python tools/claim.py <n>              claim #n: a worktree at ../Modular-<n> on issue-<n>
    python tools/claim.py <n> --existing   claim #n for the issue-<n> worktree this machine already has
    python tools/claim.py <n> --release    give #n up: delete origin/issue-<n> and the label

A claim is the branch issue-<n> on origin. It is pushed only if origin doesn't
have it yet, a check GitHub makes as part of the push, so when two machines
claim the same issue at once exactly one of them gets it. The `claimed` label
and a comment naming the machine are for people reading the issue, and let
`gh issue list --search "-label:claimed"` skip claimed issues.

What's pushed is an empty claim commit on top of the branch, which the local
branch doesn't get, so it never reaches master. To back work up to the branch
later, push over it: `git push --force-with-lease origin issue-<n>`.

The claim lasts until the branch is deleted: by `--release`, or once the work
has landed on master (`git push origin --delete issue-<n>`).

Exits 1 if the issue is claimed already, closed, or the claim fails.
"""

import datetime
import os
import socket
import subprocess
import sys
from pathlib import Path


def run(*args, check=True, cwd=None):
    result = subprocess.run(args, capture_output=True, text=True, cwd=cwd)
    if check and result.returncode != 0:
        sys.exit(f"{' '.join(args)} failed:\n{result.stderr.strip()}")
    return result


def main_checkout():
    """The first worktree git lists is the main checkout; issue worktrees sit beside it."""
    porcelain = run("git", "worktree", "list", "--porcelain").stdout
    return Path(porcelain.splitlines()[0].removeprefix("worktree "))


def on_origin(branch):
    return run("git", "ls-remote", "--exit-code", "--heads", "origin", branch, check=False).returncode == 0


def local_branch(branch):
    return run("git", "rev-parse", "--verify", "--quiet", f"refs/heads/{branch}", check=False).returncode == 0


def claim(n, existing):
    branch = f"issue-{n}"
    issue = run("gh", "issue", "view", str(n), "--json", "state,labels,title",
                "-q", '[.state, ([.labels[].name] | join(",")), .title] | join("\t")').stdout.strip()
    state, labels, title = issue.split("\t", 2)
    if state != "OPEN":
        sys.exit(f"#{n} is {state.lower()}")
    if "claimed" in labels.split(",") or on_origin(branch):
        sys.exit(f"#{n} is claimed already: see the comments on it, and origin/{branch}")

    worktree = main_checkout().parent / f"Modular-{n}"
    created = False
    if existing:
        if not local_branch(branch):
            sys.exit(f"there is no local {branch} to claim")
    else:
        # A local branch means another session on this machine has it, even
        # if it hasn't published its claim
        if local_branch(branch):
            sys.exit(f"{branch} exists here already: another session has #{n}")
        run("git", "fetch", "--quiet", "origin")
        if run("git", "worktree", "add", str(worktree), "-b", branch, "origin/master",
               check=False).returncode != 0:
            sys.exit(f"couldn't make {worktree}: another session has just claimed #{n}")
        created = True

    # The claim is an empty commit on top of the branch, pushed but kept off
    # the local branch so it never lands on master. Each claim's commit is
    # its own: git skips the lease check on a push that changes nothing, so
    # two machines pushing the same master tip would both get through
    machine = os.environ.get("COMPUTERNAME") or socket.gethostname()
    now = datetime.datetime.now().astimezone()
    today = now.date().isoformat()
    marker = run("git", "commit-tree", f"{branch}^{{tree}}", "-p", branch,
                 "-m", f"Claim #{n} from {machine} at {now.isoformat(timespec='seconds')}").stdout.strip()
    # The empty value after the colon makes the push conditional on origin not
    # having the branch yet
    push = run("git", "push", "--quiet", f"--force-with-lease=refs/heads/{branch}:", "origin",
               f"{marker}:refs/heads/{branch}", check=False)
    if push.returncode != 0:
        if created:
            run("git", "worktree", "remove", str(worktree), check=False)
            run("git", "branch", "-D", branch, check=False)
        sys.exit(f"#{n} was claimed from somewhere else first:\n{push.stderr.strip()}")

    run("gh", "issue", "edit", str(n), "--add-label", "claimed")
    run("gh", "issue", "comment", str(n), "--body",
        f"Claimed on {today} by an agent on `{machine}`, on branch `{branch}`. "
        f"Deleting the branch from origin releases the claim.")
    where = f" in {worktree}" if created else ""
    print(f"claimed #{n} ({title}){where}")


def release(n):
    branch = f"issue-{n}"
    if on_origin(branch):
        run("git", "push", "--quiet", "origin", "--delete", branch)
    state = run("gh", "issue", "view", str(n), "--json", "state", "-q", ".state").stdout.strip()
    if state == "OPEN":
        run("gh", "issue", "edit", str(n), "--remove-label", "claimed", check=False)
    print(f"released #{n} (the local worktree, if any, is left as it is)")


def main():
    args = sys.argv[1:]
    numbers = [a for a in args if a.isdigit()]
    flags = {a for a in args if not a.isdigit()}
    if len(numbers) != 1 or not flags <= {"--existing", "--release"} or len(flags) > 1:
        sys.exit(__doc__)
    n = int(numbers[0])
    if "--release" in flags:
        release(n)
    else:
        claim(n, "--existing" in flags)


if __name__ == "__main__":
    main()

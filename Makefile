# Makefile - daily entry point for CodeNotch for Windows.
#
# Run `make` (or `make help`) for the list of targets.
#
# Requirements: GNU Make 4.x for Windows (e.g. winget ezwinports.make), cargo, PowerShell 5.1+.
#
# --- Why the shell is pinned -----------------------------------------------------------------
# GNU Make for Windows runs recipes with sh.exe when it finds one on PATH and with cmd.exe
# otherwise. Launched from Git Bash it finds Git's sh.exe; launched from PowerShell it usually
# does not. The same recipe would then behave differently depending on the terminal. So the
# shell is pinned to cmd.exe below, and every recipe is written for cmd.exe only.
#
# Facts about this setup that every recipe below respects:
#   * Make writes each recipe line to a temporary .bat file and runs it. A literal % must be
#     written %% (avoid it). .SHELLFLAGS is set for completeness but batch mode ignores it.
#   * Anything beyond a plain command line is done in PowerShell, through $(PS) "...": the
#     script is inside ONE pair of double quotes, so it must never contain a double quote
#     (use single-quoted PowerShell strings) and every PowerShell $ is written $$ for make.
#   * From Git Bash, PATH starts with Git's usr\bin, so `find`, `sort`, `tar`, `link`... may
#     resolve to the GNU tools instead of the Windows ones. Recipes do not use them.
#   * The ezwinports make is a 32-bit program: the cmd.exe and powershell.exe it starts run
#     as 32-bit (WOW64) processes. Process lookups therefore use Win32_Process (see CN_PROCS).
#   * Make variables read from the environment (APPDATA, LOCALAPPDATA, CODENOTCH_DATA_DIR,
#     CARGO_TARGET_DIR) are Windows paths in both terminals.
#
# --- The hook-next-to-app rule ---------------------------------------------------------------
# codenotch.exe looks for codenotch-hook.exe in its own folder. Every build that a run, dist
# or install target depends on therefore builds BOTH binaries into the same profile folder.
#
# --- Why `stop` exists -----------------------------------------------------------------------
# Windows locks a running .exe: if codenotch.exe from target\ is running, the next cargo build
# that has to relink it fails with "Access is denied (os error 5)". Builds are NOT stopped
# for you (the app may hold state you care about). When a build fails, the Makefile checks
# whether a CodeNotch process is running from target\ and, if so, says so and suggests
# `make stop`. `install`/`uninstall` refuse to run while the installed copy is running.

SHELL := cmd.exe
.SHELLFLAGS := /C
.DEFAULT_GOAL := help
# cargo serialises builds on the target\ lock anyway; keep make output in order.
.NOTPARALLEL:
# No implicit rules or built-in variables: every target here is explicit and phony.
MAKEFLAGS += --no-builtin-rules --no-builtin-variables
.SUFFIXES:

SELF := $(lastword $(MAKEFILE_LIST))

# --- Tools -----------------------------------------------------------------------------------
CARGO ?= cargo
PS    := powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command
# Prepended to every PowerShell script:
#  - any error ends the script with a non-zero exit code;
#  - numbers use the invariant culture (a French locale would print 45 589 with a narrow
#    no-break space, which Git Bash shows as `?`);
#  - when the output is a pipe (Git Bash/mintty, or `make ... > file`) it is written as UTF-8,
#    like cargo and codenotch.exe do, instead of the OEM code page. In a real console window
#    PowerShell writes Unicode directly and nothing is changed.
PS_STRICT = $$ErrorActionPreference = 'Stop'; \
  [Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture; \
  if ([Console]::IsOutputRedirected) { [Console]::OutputEncoding = New-Object Text.UTF8Encoding $$false };

# --- Paths -----------------------------------------------------------------------------------
TARGET_DIR   := $(if $(CARGO_TARGET_DIR),$(CARGO_TARGET_DIR),target)
DEBUG_DIR    := $(TARGET_DIR)\debug
RELEASE_DIR  := $(TARGET_DIR)\release
DEBUG_EXE    := $(DEBUG_DIR)\codenotch.exe
RELEASE_EXE  := $(RELEASE_DIR)\codenotch.exe
RELEASE_HOOK := $(RELEASE_DIR)\codenotch-hook.exe

# Workspace version: the first line starting with `version =` in the root Cargo.toml, which is
# the one in [workspace.package] (dependency versions are never at the start of a line).
VERSION := $(subst ",,$(word 3,$(shell findstr /B /R /C:"version *=" Cargo.toml)))

# Same rule as the app (crates/core/src/paths.rs): CODENOTCH_DATA_DIR, else %APPDATA%\dev.kev.smashtoken.
DATA_DIR    := $(if $(CODENOTCH_DATA_DIR),$(CODENOTCH_DATA_DIR),$(APPDATA)\dev.kev.smashtoken)
LOG_FILE    := $(DATA_DIR)\logs\codenotch.log
INSTALL_DIR := $(LOCALAPPDATA)\Programs\CodeNotch

# Package names, for the test-crate error message (crates/app is the `codenotch` package).
PACKAGES = $(patsubst codenotch-app,codenotch,$(addprefix codenotch-,$(notdir $(wildcard crates/*))))

# --- Command-line switches -------------------------------------------------------------------
# Assigned with := so that only the command line can set them, never a stray environment
# variable. The `##=` lines are shown by `make help`.
##= ARGS=...       Extra arguments for the run targets, e.g. make once ARGS=--help
##= SAFE=1         Run targets set CODENOTCH_NO_SPAWN=1: the app spawns no helper CLIs
##= CRATE=name     Package for test-crate, e.g. CRATE=codenotch-core
##= FOLLOW=1       logs: keep following the log file (Ctrl+C to stop)
##= TAIL=n         logs: number of lines to show (default 50)
##= INSTALL_DIR=   install/uninstall folder (default %LOCALAPPDATA%\Programs\CodeNotch)
##= CARGO=         cargo executable (default cargo)
ARGS   :=
SAFE   :=
CRATE  :=
FOLLOW :=
TAIL   := 50

# Prefix put before the app's command line when SAFE=1 (and always for once-safe).
NO_SPAWN := $(if $(filter 1,$(SAFE)),set "CODENOTCH_NO_SPAWN=1" &&)

# =============================================================================================
# PowerShell snippets (one logical line each; make joins the continuation lines with spaces).
# In a make variable `#` starts a comment, hence `\#` where PowerShell needs a literal #.
# =============================================================================================

# help: parse this Makefile. `##@ Group` starts a group, `target: ... ## text` documents a
# target, `##= NAME=value text` documents a variable.
HELP_PS = $(PS_STRICT) \
  $$rows = foreach ($$l in Get-Content -LiteralPath '$(SELF)') { \
    if ($$l -match '^\#\#@\s*(.+)$$') { [pscustomobject]@{ Kind = 'group'; Name = $$Matches[1]; Text = '' } } \
    elseif ($$l -match '^([A-Za-z][\w.-]*)\s*:[^=\#]*\#\#\s*(.+)$$') { [pscustomobject]@{ Kind = 'target'; Name = $$Matches[1]; Text = $$Matches[2] } } \
    elseif ($$l -match '^\#\#=\s*(\S+)\s+(.+)$$') { [pscustomobject]@{ Kind = 'var'; Name = $$Matches[1]; Text = $$Matches[2] } } \
  }; \
  $$w = ($$rows | Where-Object { $$_.Kind -ne 'group' } | ForEach-Object { $$_.Name.Length } | Measure-Object -Maximum).Maximum; \
  Write-Host 'CodeNotch for Windows $(VERSION)'; \
  Write-Host 'Usage: make <target> [NAME=value ...]'; \
  foreach ($$r in $$rows) { \
    if ($$r.Kind -eq 'group') { Write-Host ''; Write-Host $$r.Name -ForegroundColor Yellow } \
    elseif ($$r.Kind -eq 'target') { Write-Host ('  ' + $$r.Name.PadRight($$w)) -ForegroundColor Cyan -NoNewline; Write-Host ('  ' + $$r.Text) } \
  }; \
  Write-Host ''; Write-Host 'Variables' -ForegroundColor Yellow; \
  foreach ($$r in $$rows | Where-Object { $$_.Kind -eq 'var' }) { Write-Host ('  ' + $$r.Name.PadRight($$w)) -ForegroundColor Cyan -NoNewline; Write-Host ('  ' + $$r.Text) }

# Running CodeNotch processes, with their full path. Win32_Process (CIM) is used rather than
# Get-Process: the ezwinports make is a 32-bit program, so the PowerShell it starts is 32-bit
# too, and Get-Process cannot read the path of a 64-bit process from there (Path is empty).
# (Inside a single-quoted PowerShell string, '' is one quote.)
CN_PROCS = @(Get-CimInstance Win32_Process -Filter 'Name=''codenotch.exe'' OR Name=''codenotch-hook.exe''')

# After a failed build: is a CodeNotch exe running from target\ (and so locking its file)?
LOCK_HINT_PS = $(PS_STRICT) \
  $$t = [IO.Path]::GetFullPath('$(TARGET_DIR)').TrimEnd('\') + '\'; \
  $(CN_PROCS) | \
  Where-Object { $$_.ExecutablePath -and $$_.ExecutablePath.StartsWith($$t, [StringComparison]::OrdinalIgnoreCase) } | \
  ForEach-Object { Write-Host ('note: ' + $$_.ExecutablePath + ' is running (PID ' + $$_.ProcessId + ') and locks its file, which makes the link step fail. Close it or run: make stop') -ForegroundColor Yellow }

STOP_PS = $(PS_STRICT) \
  $$p = @($(CN_PROCS) | Where-Object { $$_.Name -eq 'codenotch.exe' }); \
  if ($$p.Count -eq 0) { Write-Host 'codenotch.exe is not running.'; exit 0 }; \
  foreach ($$x in $$p) { Write-Host ('Stopping codenotch.exe PID ' + $$x.ProcessId + ' (' + $$x.ExecutablePath + ')'); Stop-Process -Id $$x.ProcessId -Force }; \
  Wait-Process -Id ($$p | ForEach-Object { $$_.ProcessId }) -Timeout 10 -ErrorAction SilentlyContinue; \
  exit 0

LOGS_PS = $(PS_STRICT) \
  $$f = '$(LOG_FILE)'; \
  if (-not (Test-Path -LiteralPath $$f)) { Write-Host ('No log file yet: ' + $$f); exit 0 }; \
  Write-Host ('==> ' + $$f) -ForegroundColor Yellow; \
  Get-Content -LiteralPath $$f -Encoding UTF8 -Tail $(TAIL) $(if $(filter 1,$(FOLLOW)),-Wait)

SIZE_PS = $(PS_STRICT) \
  $$missing = $$false; \
  foreach ($$f in '$(RELEASE_EXE)', '$(RELEASE_HOOK)') { \
    if (Test-Path -LiteralPath $$f) { $$n = (Get-Item -LiteralPath $$f).Length; '{0,-40} {1,12:N0} bytes  {2,7:N2} MB' -f $$f, $$n, ($$n / 1MB) } \
    else { '{0,-40} missing' -f $$f; $$missing = $$true } \
  }; \
  if ($$missing) { Write-Host 'Build them with: make release'; exit 1 }

LOC_PS = $(PS_STRICT) \
  $$rows = foreach ($$d in Get-ChildItem -LiteralPath 'crates' -Directory) { \
    $$files = @(Get-ChildItem -LiteralPath $$d.FullName -Recurse -File -Filter '*.rs'); $$n = 0; \
    foreach ($$f in $$files) { $$n += [IO.File]::ReadAllLines($$f.FullName).Length }; \
    [pscustomobject]@{ Crate = $$d.Name; Files = $$files.Count; Lines = $$n } \
  }; \
  '{0,-18} {1,6} {2,9}' -f 'crate', 'files', 'lines'; \
  foreach ($$r in $$rows | Sort-Object Lines -Descending) { '{0,-18} {1,6} {2,9:N0}' -f $$r.Crate, $$r.Files, $$r.Lines }; \
  '{0,-18} {1,6} {2,9:N0}' -f 'total', ($$rows | Measure-Object Files -Sum).Sum, ($$rows | Measure-Object Lines -Sum).Sum

# dist and update-manifest run scripts\dist.ps1 and scripts\update-manifest.ps1, which the
# release workflow (.github\workflows\release.yml) runs too: one packaging path everywhere.
PS_FILE := powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File

# Fails if a CodeNotch exe is running from INSTALL_DIR (its files would be locked).
INSTALL_BUSY_PS = \
  $$full = [IO.Path]::GetFullPath($$d).TrimEnd('\'); \
  $$busy = @($(CN_PROCS) | Where-Object { $$_.ExecutablePath -and ([IO.Path]::GetDirectoryName($$_.ExecutablePath) -ieq $$full) }); \
  if ($$busy.Count -gt 0) { Write-Host ('error: ' + $$busy[0].ExecutablePath + ' is running (PID ' + $$busy[0].ProcessId + '). Quit it or run: make stop') -ForegroundColor Red; exit 1 };

INSTALL_PS = $(PS_STRICT) \
  $$d = '$(INSTALL_DIR)'; $(INSTALL_BUSY_PS) \
  New-Item -ItemType Directory -Force -Path $$d | Out-Null; \
  Copy-Item -LiteralPath '$(RELEASE_EXE)', '$(RELEASE_HOOK)' -Destination $$d -Force; \
  Write-Host ('Installed CodeNotch $(VERSION) to ' + $$d + ':'); \
  Get-ChildItem -LiteralPath $$d -Filter '*.exe' | ForEach-Object { '  ' + $$_.FullName }; \
  Write-Host 'No shortcut, registry or autostart entry was created; the data folder was not touched.'

# Removes the two exes, then the folder only if nothing else is left in it.
UNINSTALL_PS = $(PS_STRICT) \
  $$d = '$(INSTALL_DIR)'; \
  if (-not (Test-Path -LiteralPath $$d)) { Write-Host ('Nothing to remove: ' + $$d + ' does not exist.'); exit 0 }; \
  $(INSTALL_BUSY_PS) \
  foreach ($$n in 'codenotch.exe', 'codenotch-hook.exe') { $$f = Join-Path $$d $$n; if (Test-Path -LiteralPath $$f) { Remove-Item -LiteralPath $$f -Force; Write-Host ('Removed ' + $$f) } }; \
  if (@(Get-ChildItem -LiteralPath $$d -Force).Count -eq 0) { Remove-Item -LiteralPath $$d -Force; Write-Host ('Removed ' + $$d) } \
  else { Write-Host ('Left ' + $$d + ' in place: it contains other files.') -ForegroundColor Yellow }; \
  Write-Host 'The data folder, the registry and autostart entries were not touched.'

# $(call cargo-build,<cargo build arguments>): echo the command, run it, and on failure point
# at a running exe that locks its output file.
define cargo-build
@echo $(CARGO) build $(1)
@$(CARGO) build $(1) || ($(PS) "$(LOCK_HINT_PS)" & exit 1)
endef

# =============================================================================================
# Targets
# =============================================================================================

.PHONY: help build release hook check \
        demo run dev run-release settings once once-safe doctor version \
        test test-crate lint fmt fmt-check ci \
        dist update-manifest install uninstall \
        stop logs data-dir clean size loc

##@ Build
build: ## Debug build of the whole workspace (app + hook in target\debug)
	$(call cargo-build,--workspace)

release: ## Release build of codenotch.exe and codenotch-hook.exe (target\release)
	$(call cargo-build,--release -p codenotch -p codenotch-hook)

hook: ## Debug build of codenotch-hook.exe only
	$(call cargo-build,-p codenotch-hook)

check: ## cargo check --workspace (fast type check, no exe)
	$(CARGO) check --workspace

##@ Run   (debug build first; ARGS=... adds arguments, SAFE=1 sets CODENOTCH_NO_SPAWN=1)
demo: build ## Open the notch on fake data (--demo): no engine, no network
	$(NO_SPAWN) "$(DEBUG_EXE)" --demo $(ARGS)

run: build ## Start the real app (debug build)
	$(NO_SPAWN) "$(DEBUG_EXE)" $(ARGS)

dev: ## Watch crates\, Cargo.toml/.lock, .cargo\: rebuild and restart the debug app on save (Ctrl+C to stop)
	powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File scripts\dev.ps1 -Cargo "$(CARGO)" -TargetDir "$(TARGET_DIR)" -AppArgs "$(ARGS)" $(if $(filter 1,$(SAFE)),-Safe)

run-release: release ## Start the real app (release build)
	$(NO_SPAWN) "$(RELEASE_EXE)" $(ARGS)

settings: build ## Start the app with the settings window open (--settings)
	$(NO_SPAWN) "$(DEBUG_EXE)" --settings $(ARGS)


once: build ## Fetch every enabled account once and print it (--once, read-only)
	$(NO_SPAWN) "$(DEBUG_EXE)" --once $(ARGS)

once-safe: NO_SPAWN := set "CODENOTCH_NO_SPAWN=1" &&
once-safe: build ## Same as once, with CODENOTCH_NO_SPAWN=1 (no helper CLI spawned)
	$(NO_SPAWN) "$(DEBUG_EXE)" --once $(ARGS)

doctor: build ## Print the diagnostics report (data folder, config, providers, hooks)
	$(NO_SPAWN) "$(DEBUG_EXE)" doctor $(ARGS)

version: build ## Print the app version (--version)
	$(NO_SPAWN) "$(DEBUG_EXE)" --version $(ARGS)

##@ Quality
test: ## Run every test of the workspace
	$(CARGO) test --workspace

test-crate: ## Test one package: make test-crate CRATE=codenotch-core
	$(if $(CRATE),,$(error CRATE is not set. Usage: make test-crate CRATE=<package>. Packages: $(PACKAGES)))
	$(CARGO) test -p $(CRATE)

lint: ## Clippy on every target, warnings are errors
	$(CARGO) clippy --workspace --all-targets -- -D warnings

fmt: ## Format all the code (rewrites files)
	$(CARGO) fmt --all

fmt-check: ## Check formatting without changing files
	$(CARGO) fmt --all -- --check

ci: fmt-check lint test ## fmt-check + lint + test: what must pass before a commit
	@echo ci: fmt-check, lint and test passed.

##@ Package
dist: release ## dist\CodeNotch-<version>\ + .zip (both exes, LICENSE, README, glyph NOTICE)
	$(if $(VERSION),,$(error Could not read the workspace version from Cargo.toml))
	@$(PS_FILE) scripts\dist.ps1 -Version "$(VERSION)" -TargetDir "$(TARGET_DIR)"

update-manifest: ## Write update.json from dist\CodeNotch-<version>.zip (run make dist first)
	$(if $(VERSION),,$(error Could not read the workspace version from Cargo.toml))
	@$(PS_FILE) scripts\update-manifest.ps1 -Version "$(VERSION)"

install: release ## Copy both release exes to %LOCALAPPDATA%\Programs\CodeNotch (no admin)
	$(if $(LOCALAPPDATA),,$(error LOCALAPPDATA is not set))
	@$(PS) "$(INSTALL_PS)"

uninstall: ## Remove the installed exes and their folder (not the data, registry or autostart)
	$(if $(LOCALAPPDATA),,$(error LOCALAPPDATA is not set))
	@$(PS) "$(UNINSTALL_PS)"

##@ Maintenance
help: ## Show this list (default target)
	@$(PS) "$(HELP_PS)"

stop: ## Kill every running codenotch.exe (a running exe makes the next build fail to link)
	@$(PS) "$(STOP_PS)"

logs: ## Show the end of the app log (TAIL=50 lines, FOLLOW=1 to keep following)
	@$(PS) "$(LOGS_PS)"

data-dir: ## Print the data folder path (CODENOTCH_DATA_DIR or %APPDATA%\dev.kev.smashtoken)
	@echo $(DATA_DIR)

clean: ## cargo clean + remove dist\ (the next build starts from scratch)
	$(CARGO) clean
	if exist dist rmdir /s /q dist

size: ## Print the size of the two release exes
	@$(PS) "$(SIZE_PS)"

loc: ## Lines of Rust per crate
	@$(PS) "$(LOC_PS)"

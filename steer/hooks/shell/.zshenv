#!/bin/zsh
# Installed by Rust Spice; do not edit.
#
# Agent tools do not consistently honor SHELL, but zsh always reads .zshenv.
# Reenter the exact wrapper once while the complete execution string still
# exists, then let the static stage restore ordinary descendant semantics.

if [ -n "${ZSH_EXECUTION_STRING-}" ]; then
  _spice_shell_bin=${ZSH_NAME:-zsh}
  if ! _spice_shell_bin=$(command -v "$_spice_shell_bin"); then
    printf '%s\n' 'rust spice shell hook: cannot resolve zsh' >&2
    exit 127
  fi
  if [[ -o login ]]; then
    _spice_shell_mode=-lc
  else
    _spice_shell_mode=-c
  fi
  unset ZDOTDIR BASH_ENV
  SPICE_RS_NATIVE_SHELL=$_spice_shell_bin
  export SPICE_RS_NATIVE_SHELL
  exec "${SPICE_RS_WRAPPER_SPICE:?rust spice executable}" run -- \
    "${SPICE_RS_HOOKS:?rust spice hooks directory}/static/stage.sh" \
    "$_spice_shell_mode" "$ZSH_EXECUTION_STRING"
  printf '%s\n' 'rust spice shell hook: failed to exec wrapper' >&2
  exit 127
fi

# An interactive shell has no complete command to wrap. Restore normal startup
# lookup before zsh advances to the remaining user startup files.
unset ZDOTDIR BASH_ENV

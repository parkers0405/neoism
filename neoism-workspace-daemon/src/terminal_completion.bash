# Isolated worker. Positional arguments are data; neither query nor alias text
# is evaluated. Source only the daemon's private Bash-generated state snapshot.
ulimit -t 2
# Do not cap virtual address space: V8/Go reserve large ranges with low RSS.
# Wall time, CPU, output, producer rows and concurrent process groups are bounded.
ulimit -c 0
shopt -s extglob
if [[ -n $1 ]]; then source "$1" >/dev/null 2>&1 || exit 3; fi
trap - DEBUG
set +e +u
IFS=$' \t\n'
__neoism_worker_state=$1
COMP_LINE=$2 COMP_POINT=$3 COMP_CWORD=$4 COMP_TYPE=9 COMP_KEY=9
shift 4
COMP_WORDS=("$@")
__neoism_worker_rows=0 __neoism_worker_bytes=0 __neoism_worker_seen=''

# Bound records while consuming producers. Duplicate records do not consume
# quota. Closing a bounded loop's process-substitution pipe stops its producer.
__neoism_worker_emit() {
  local LC_ALL=C __neoism_worker_key __neoism_worker_size
  [[ -z $2 || ${#2} -gt 4096 || $2 == *[[:cntrl:]]* ]] && return 0
  __neoism_worker_key=$'\036'"$1"$'\037'"$2"$'\036'
  [[ $__neoism_worker_seen == *"$__neoism_worker_key"* ]] && return 0
  __neoism_worker_size=$(( ${#1} + ${#2} + ${#3} + 3 ))
  (( __neoism_worker_rows >= 512 || __neoism_worker_bytes + __neoism_worker_size > 200000 )) && return 1
  builtin printf '%s\0%s\0%s\0' "$1" "$2" "$3"
  __neoism_worker_seen+=$__neoism_worker_key
  (( __neoism_worker_rows++ )); (( __neoism_worker_bytes += __neoism_worker_size ))
  return 0
}

# Literal lexer, not eval: quotes/escapes are supported; substitutions,
# operators, globs, executable ! aliases, and incomplete quoting are rejected.
__neoism_worker_literal_words() {
  local LC_ALL=C __neoism_worker_text=$1 __neoism_worker_char __neoism_worker_next
  local __neoism_worker_quote='' __neoism_worker_word='' __neoism_worker_active=0 __neoism_worker_escape=0 __neoism_worker_i
  __neoism_worker_words=()
  (( ${#1} <= 8192 )) || return 1
  for (( __neoism_worker_i=0; __neoism_worker_i<${#__neoism_worker_text}; __neoism_worker_i++ )); do
    __neoism_worker_char=${__neoism_worker_text:__neoism_worker_i:1}
    [[ $__neoism_worker_char == *[[:cntrl:]]* ]] && return 1
    if (( __neoism_worker_escape )); then
      __neoism_worker_word+=$__neoism_worker_char; __neoism_worker_escape=0; continue
    fi
    if [[ $__neoism_worker_quote == "'" ]]; then
      if [[ $__neoism_worker_char == "'" ]]; then __neoism_worker_quote=''; else __neoism_worker_word+=$__neoism_worker_char; fi
      continue
    fi
    if [[ $__neoism_worker_char == '\' ]]; then
      __neoism_worker_next=${__neoism_worker_text:__neoism_worker_i+1:1}
      if [[ $__neoism_worker_quote == '"' && $__neoism_worker_next != [\$\`\"\\] ]]; then
        __neoism_worker_word+='\'
      else __neoism_worker_escape=1; fi
      __neoism_worker_active=1; continue
    fi
    if [[ -n $__neoism_worker_quote ]]; then
      [[ $__neoism_worker_char == '$' || $__neoism_worker_char == '`' ]] && return 1
      if [[ $__neoism_worker_char == '"' ]]; then __neoism_worker_quote=''; else __neoism_worker_word+=$__neoism_worker_char; fi
      continue
    fi
    case $__neoism_worker_char in
      "'"|'"') __neoism_worker_quote=$__neoism_worker_char; __neoism_worker_active=1 ;;
      ' ') if (( __neoism_worker_active )); then __neoism_worker_words+=("$__neoism_worker_word"); fi; __neoism_worker_word=''; __neoism_worker_active=0 ;;
      '$'|'`'|';'|'|'|'&'|'<'|'>'|'('|')'|'{'|'}'|'*'|'?'|'['|']'|'~'|'!') return 1 ;;
      *) __neoism_worker_word+=$__neoism_worker_char; __neoism_worker_active=1 ;;
    esac
    (( ${#__neoism_worker_words[@]} <= 4096 )) || return 1
  done
  [[ -z $__neoism_worker_quote && $__neoism_worker_escape == 0 ]] || return 1
  (( __neoism_worker_active )) && __neoism_worker_words+=("$__neoism_worker_word")
  (( ${#__neoism_worker_words[@]} > 0 ))
}

# Rebuild only the completer's context; Rust retains the original line/spans for
# insertion. LC_ALL=C makes COMP_POINT a byte offset even with Unicode aliases.
__neoism_worker_context_line() {
  local LC_ALL=C __neoism_worker_i __neoism_worker_quoted
  COMP_LINE=''
  for (( __neoism_worker_i=0; __neoism_worker_i<${#COMP_WORDS[@]}; __neoism_worker_i++ )); do
    (( __neoism_worker_i > 0 )) && COMP_LINE+=' '
    __neoism_worker_quoted=''
    [[ -n ${COMP_WORDS[__neoism_worker_i]} ]] && builtin printf -v __neoism_worker_quoted '%q' "${COMP_WORDS[__neoism_worker_i]}"
    COMP_LINE+=$__neoism_worker_quoted
    (( __neoism_worker_i == COMP_CWORD )) && COMP_POINT=${#COMP_LINE}
  done
}

__neoism_worker_shell_alias() {
  local __neoism_worker_decl __neoism_worker_text __neoism_worker_depth __neoism_worker_changed=0
  local -a __neoism_worker_prefix=("${COMP_WORDS[0]}")
  complete -p -- "${COMP_WORDS[0]}" &>/dev/null && return 0
  for (( __neoism_worker_depth=0; __neoism_worker_depth<8; __neoism_worker_depth++ )); do
    __neoism_worker_decl=$(builtin alias -- "${__neoism_worker_prefix[0]}" 2>/dev/null) || break
    __neoism_worker_literal_words "${__neoism_worker_decl#*=}" || return 0
    (( ${#__neoism_worker_words[@]} == 1 )) || return 0
    __neoism_worker_text=${__neoism_worker_words[0]}
    __neoism_worker_literal_words "$__neoism_worker_text" || return 0
    (( ${#__neoism_worker_words[@]} <= 64 )) || return 0
    __neoism_worker_prefix=("${__neoism_worker_words[@]}" "${__neoism_worker_prefix[@]:1}")
    __neoism_worker_changed=1
  done
  (( __neoism_worker_changed && __neoism_worker_depth < 8 )) || return 0
  COMP_WORDS=("${__neoism_worker_prefix[@]}" "${COMP_WORDS[@]:1}")
  (( COMP_CWORD += ${#__neoism_worker_prefix[@]} - 1 ))
  __neoism_worker_context_line
}

__neoism_worker_git() { command git -c core.fsmonitor=false -c core.hooksPath=/dev/null "$@" 2>/dev/null; }
__neoism_worker_git_alias() {
  local __neoism_worker_text __neoism_worker_depth
  local -a __neoism_worker_original_words=("${COMP_WORDS[@]}")
  local __neoism_worker_original_cword=$COMP_CWORD
  for (( __neoism_worker_depth=0; __neoism_worker_depth<8; __neoism_worker_depth++ )); do
    case ${COMP_WORDS[1]} in switch|checkout|branch|merge|rebase|log|diff|reset|show|cherry-pick|restore|fetch|pull|push|remote|ls-remote|add|stage|status|rm|mv|stash) break ;; esac
    __neoism_worker_text=$(__neoism_worker_git config --get "alias.${COMP_WORDS[1]}") || break
    if ! __neoism_worker_literal_words "$__neoism_worker_text"; then
      COMP_WORDS=("${__neoism_worker_original_words[@]}"); COMP_CWORD=$__neoism_worker_original_cword
      return 0
    fi
    (( ${#__neoism_worker_words[@]} <= 64 )) || return 0
    COMP_WORDS=("${COMP_WORDS[0]}" "${__neoism_worker_words[@]}" "${COMP_WORDS[@]:2}")
    (( COMP_CWORD += ${#__neoism_worker_words[@]} - 1 ))
  done
  if (( __neoism_worker_depth == 8 )); then
    COMP_WORDS=("${__neoism_worker_original_words[@]}"); COMP_CWORD=$__neoism_worker_original_cword
  fi
  __neoism_worker_context_line
}

__neoism_worker_main() {
  # Private bookkeeping names cannot clobber a custom completer's ordinary
  # globals, including non-exported arrays named cur/cmd/choices.
  local __neoism_worker_cur __neoism_worker_cmd __neoism_worker_prev='' __neoism_worker_spec __neoism_worker_item
  local __neoism_worker_kind __neoism_worker_fn __neoism_worker_i __neoism_worker_start __neoism_worker_search __neoism_worker_action __neoism_worker_status __neoism_worker_original
  local __neoism_worker_nospace=0
  local -a __neoism_worker_flags
  if (( COMP_CWORD > 0 )) && [[ -n $__neoism_worker_state ]]; then __neoism_worker_shell_alias; fi
  if [[ ${COMP_WORDS[0]} == git ]] && (( COMP_CWORD > 1 )); then __neoism_worker_git_alias; fi
  __neoism_worker_cur=${COMP_WORDS[COMP_CWORD]}; __neoism_worker_cmd=${COMP_WORDS[0]}
  (( COMP_CWORD > 0 )) && __neoism_worker_prev=${COMP_WORDS[COMP_CWORD-1]}
  if (( COMP_CWORD == 0 )); then
    while IFS= read -r __neoism_worker_item; do
      [[ $__neoism_worker_item == __neoism* ]] && continue
      __neoism_worker_kind=command
      if builtin alias -- "$__neoism_worker_item" &>/dev/null; then __neoism_worker_kind=alias
      elif declare -F "$__neoism_worker_item" &>/dev/null; then __neoism_worker_kind=function; fi
      __neoism_worker_emit "$__neoism_worker_kind" "$__neoism_worker_item" 'Live shell / PATH' || break
      (( __neoism_worker_rows >= 200 )) && break
    done < <(compgen -c -- "$__neoism_worker_cur")
  fi
  if [[ $__neoism_worker_cur == '$'* ]]; then
    for __neoism_worker_item in "${__neoism_live_variables[@]}"; do
      [[ $__neoism_worker_item == "${__neoism_worker_cur:1}"* && $__neoism_worker_item != __neoism* ]] || continue
      __neoism_worker_emit variable "$__neoism_worker_item" 'Live shell variable name' || break
    done
  fi
  if (( COMP_CWORD > 0 )) && [[ -n $__neoism_worker_state ]]; then
    if ! complete -p -- "$__neoism_worker_cmd" &>/dev/null && declare -F _completion_loader &>/dev/null; then
      _completion_loader "$__neoism_worker_cmd" >/dev/null 2>&1 || :
    fi
    __neoism_worker_spec=$(complete -p -- "$__neoism_worker_cmd" 2>/dev/null || complete -p -D 2>/dev/null)
    if __neoism_worker_literal_words "$__neoism_worker_spec"; then
      __neoism_worker_flags=("${__neoism_worker_words[@]}")
      for (( __neoism_worker_i=0; __neoism_worker_i<${#__neoism_worker_flags[@]}; __neoism_worker_i++ )); do
        [[ ${__neoism_worker_flags[__neoism_worker_i]} == -o && ${__neoism_worker_flags[__neoism_worker_i+1]} == nospace ]] && __neoism_worker_nospace=1
      done
      for (( __neoism_worker_i=0; __neoism_worker_i<${#__neoism_worker_flags[@]}; __neoism_worker_i++ )); do
        if [[ ${__neoism_worker_flags[__neoism_worker_i]} == -W ]]; then
          if __neoism_worker_literal_words "${__neoism_worker_flags[__neoism_worker_i+1]}"; then
            for __neoism_worker_item in "${__neoism_worker_words[@]}"; do
              [[ $__neoism_worker_item == "$__neoism_worker_cur"* ]] || continue
              __neoism_worker_kind=argument; [[ $__neoism_worker_item == -* ]] && __neoism_worker_kind=option
              __neoism_worker_emit "$__neoism_worker_kind" "$__neoism_worker_item" 'Registered Bash completion word list' || break
            done
          fi
        elif [[ ${__neoism_worker_flags[__neoism_worker_i]} == -f || ${__neoism_worker_flags[__neoism_worker_i]} == -d ]]; then
          while IFS= read -r __neoism_worker_item; do
            __neoism_worker_kind=file
            [[ -d $__neoism_worker_item ]] && { __neoism_worker_kind=directory; __neoism_worker_item=${__neoism_worker_item%/}/; }
            __neoism_worker_emit "$__neoism_worker_kind" "$__neoism_worker_item" 'Registered Bash filename completion' || break
          done < <(compgen "${__neoism_worker_flags[__neoism_worker_i]}" -- "$__neoism_worker_cur")
        elif [[ ${__neoism_worker_flags[__neoism_worker_i]} == -F ]]; then
          __neoism_worker_fn=${__neoism_worker_flags[__neoism_worker_i+1]}; COMPREPLY=()
          if declare -F "$__neoism_worker_fn" &>/dev/null; then
            "$__neoism_worker_fn" "$__neoism_worker_cmd" "$__neoism_worker_cur" "$__neoism_worker_prev" >/dev/null 2>&1 || :
            __neoism_worker_start=$__neoism_worker_rows
            for __neoism_worker_item in "${COMPREPLY[@]}"; do
              # Git and other nospace completers append their own word delimiter.
              # Keep literal trailing spaces in real paths and escaped spaces.
              if (( __neoism_worker_nospace )) && [[ $__neoism_worker_item == *' ' && $__neoism_worker_item != *'\ ' && ! -e $__neoism_worker_item && ! -L $__neoism_worker_item ]]; then
                __neoism_worker_item=${__neoism_worker_item% }
              fi
              __neoism_worker_kind=argument; [[ $__neoism_worker_item == -* ]] && __neoism_worker_kind=option
              [[ -f $__neoism_worker_item ]] && __neoism_worker_kind=file
              [[ -d $__neoism_worker_item ]] && { __neoism_worker_kind=directory; __neoism_worker_item=${__neoism_worker_item%/}/; }
              __neoism_worker_emit "$__neoism_worker_kind" "$__neoism_worker_item" 'Installed Bash completion function' || break
              (( __neoism_worker_rows - __neoism_worker_start >= 200 )) && break
            done
          fi
        fi
      done
    fi
  fi
  if [[ $__neoism_worker_cmd == git ]] && (( COMP_CWORD > 0 )); then
    __neoism_worker_action=${COMP_WORDS[1]}
    if (( COMP_CWORD > 1 )); then
      case $__neoism_worker_action in switch|checkout|branch|merge|rebase|log|diff|reset|show|cherry-pick|restore)
        __neoism_worker_start=$__neoism_worker_rows
        while IFS= read -r __neoism_worker_item; do
          [[ $__neoism_worker_item == "$__neoism_worker_cur"* ]] || continue
          __neoism_worker_emit branch "$__neoism_worker_item" 'Git branch' || break
          (( __neoism_worker_rows - __neoism_worker_start >= 128 )) && break
        done < <(__neoism_worker_git for-each-ref --format='%(refname:short)' refs/heads refs/remotes) ;; esac
      case $__neoism_worker_action in fetch|pull|push|remote|ls-remote)
        while IFS= read -r __neoism_worker_item; do
          [[ $__neoism_worker_item == "$__neoism_worker_cur"* ]] || continue
          __neoism_worker_emit remote "$__neoism_worker_item" 'Git remote' || break
        done < <(__neoism_worker_git remote) ;; esac
    fi
    if (( COMP_CWORD == 1 )); then
      while IFS= read -r __neoism_worker_item; do
        [[ $__neoism_worker_item == "$__neoism_worker_cur"* ]] || continue
        __neoism_worker_emit argument "$__neoism_worker_item" 'Installed Git subcommand' || break
      done < <(__neoism_worker_git --list-cmds=main,others,alias)
    fi
    case $__neoism_worker_action in add|stage|restore|checkout|diff|status|rm|mv|stash)
      while IFS= read -r -d '' __neoism_worker_item; do
        __neoism_worker_status=${__neoism_worker_item:0:2}; __neoism_worker_item=${__neoism_worker_item:3}
        [[ $__neoism_worker_item != "$__neoism_worker_cur"* ]] || __neoism_worker_emit file "$__neoism_worker_item" 'Git working-tree change' || break
        [[ $__neoism_worker_status == *R* || $__neoism_worker_status == *C* ]] && IFS= read -r -d '' __neoism_worker_original
      done < <(__neoism_worker_git status --porcelain=v1 -z --untracked-files=normal) ;; esac
  fi
  __neoism_worker_search=$__neoism_worker_cur
  [[ $__neoism_worker_cur == '~/'* ]] && __neoism_worker_search=$HOME/${__neoism_worker_cur:2}
  __neoism_worker_start=$__neoism_worker_rows
  while IFS= read -r __neoism_worker_item; do
    __neoism_worker_kind=file
    if [[ -d $__neoism_worker_item ]]; then __neoism_worker_kind=directory; __neoism_worker_item=${__neoism_worker_item%/}/; fi
    [[ $__neoism_worker_cmd == cd && $__neoism_worker_kind != directory && $COMP_CWORD != 0 ]] && continue
    __neoism_worker_emit "$__neoism_worker_kind" "$__neoism_worker_item" 'Host filesystem' || break
    (( __neoism_worker_rows - __neoism_worker_start >= 200 )) && break
  done < <(compgen -f -- "$__neoism_worker_search")
  return 0
}
__neoism_worker_main
exit 0
local M = {}

local current_theme = "pastel_dark"

local palettes = {
  pastel_dark = {
    bg = "#000000",
    fg = "#e8e8e8",
    line = "#1a1a1a",
    surface = "#1f1f1f",
    muted = "#5a5a5a",
    comment = "#7a7a7a",
    string = "#9fe8c3",
    number = "#eda685",
    keyword = "#c2a2e3",
    statement = "#c2a2e3",
    func = "#99aee5",
    type = "#fbdf90",
    property = "#99aee5",
    constructor = "#b5c3ea",
    special = "#ef8891",
    error = "#ef8891",
    warn = "#eda685",
    info = "#99aee5",
  },
  nvchad_one = {
    bg = "#1e222a",
    fg = "#abb2bf",
    line = "#252931",
    surface = "#282c34",
    muted = "#565c64",
    comment = "#565c64",
    string = "#98c379",
    number = "#d19a66",
    keyword = "#c678dd",
    statement = "#e06c75",
    func = "#61afef",
    type = "#e5c07b",
    property = "#e06c75",
    constructor = "#56b6c2",
    special = "#be5046",
    error = "#e06c75",
    warn = "#e7c787",
    info = "#61afef",
    base16 = {
      base00 = "#1e222a",
      base01 = "#353b45",
      base02 = "#3e4451",
      base03 = "#545862",
      base04 = "#565c64",
      base05 = "#abb2bf",
      base06 = "#b6bdca",
      base07 = "#c8ccd4",
      base08 = "#e06c75",
      base09 = "#d19a66",
      base0A = "#e5c07b",
      base0B = "#98c379",
      base0C = "#56b6c2",
      base0D = "#61afef",
      base0E = "#c678dd",
      base0F = "#be5046",
    },
  },
  tokyo_night = {
    bg = "#1a1b26",
    fg = "#c0caf5",
    line = "#16161e",
    surface = "#24283b",
    muted = "#565f89",
    comment = "#565f89",
    string = "#9ece6a",
    number = "#ff9e64",
    keyword = "#7aa2f7",
    statement = "#bb9af7",
    func = "#7aa2f7",
    type = "#2ac3de",
    property = "#73daca",
    constructor = "#7dcfff",
    special = "#e0af68",
    error = "#f7768e",
    warn = "#e0af68",
    info = "#7dcfff",
  },
  catppuccin_mocha = {
    bg = "#1e1e2e",
    fg = "#cdd6f4",
    line = "#11111b",
    surface = "#313244",
    muted = "#6c7086",
    comment = "#6c7086",
    string = "#a6e3a1",
    number = "#fab387",
    keyword = "#89b4fa",
    statement = "#cba6f7",
    func = "#f9e2af",
    type = "#94e2d5",
    property = "#89dceb",
    constructor = "#89dceb",
    special = "#f5c2e7",
    error = "#f38ba8",
    warn = "#f9e2af",
    info = "#89b4fa",
  },
}

local base_groups = {
  Normal = { bg = "#000000", fg = "#e8e8e8" },
  NormalNC = { bg = "#000000", fg = "#e8e8e8" },
  NormalFloat = { bg = "#000000", fg = "#e8e8e8" },
  FloatBorder = { bg = "#000000", fg = "#1f1f1f" },
  EndOfBuffer = { bg = "#000000", fg = "#000000" },
  SignColumn = { bg = "#000000" },
  FoldColumn = { bg = "#000000", fg = "#5a5a5a" },
  LineNr = { bg = "#000000", fg = "#5a5a5a" },
  CursorLine = { bg = "#0a0a0a" },
  CursorLineNr = { bg = "#0a0a0a", fg = "#e8e8e8", bold = true },
  Visual = { bg = "#1f1f1f" },
  VisualNOS = { bg = "#1f1f1f" },
  Search = { bg = "#3d2e00", fg = "#e8e8e8" },
  IncSearch = { bg = "#665100", fg = "#000000" },
  MatchParen = { bg = "#1f1f1f", fg = "#e8e8e8", bold = true },
  Pmenu = { bg = "#0a0a0a", fg = "#e8e8e8" },
  PmenuSel = { bg = "#1f1f1f", fg = "#e8e8e8" },
  PmenuSbar = { bg = "#0a0a0a" },
  PmenuThumb = { bg = "#1f1f1f" },
  StatusLine = { bg = "#000000", fg = "#e8e8e8" },
  StatusLineNC = { bg = "#000000", fg = "#5a5a5a" },
  WinSeparator = { bg = "#000000", fg = "#1f1f1f" },
  VertSplit = { bg = "#000000", fg = "#1f1f1f" },
  TabLine = { bg = "#000000", fg = "#7a7a7a" },
  TabLineSel = { bg = "#1f1f1f", fg = "#e8e8e8" },
  TabLineFill = { bg = "#000000" },
  Cursor = { bg = "#e8e8e8", fg = "#000000" },
  lCursor = { bg = "#e8e8e8", fg = "#000000" },
  TermCursor = { bg = "#e8e8e8" },
}

local syntax_groups = {
  Comment = { fg = "#6a9955", italic = true },
  Constant = { fg = "#b5cea8" },
  String = { fg = "#ce9178" },
  Character = { fg = "#ce9178" },
  Number = { fg = "#b5cea8" },
  Boolean = { fg = "#569cd6" },
  Float = { fg = "#b5cea8" },
  Identifier = { fg = "#9cdcfe" },
  Function = { fg = "#dcdcaa" },
  Statement = { fg = "#c586c0" },
  Conditional = { fg = "#c586c0" },
  Repeat = { fg = "#c586c0" },
  Label = { fg = "#c586c0" },
  Operator = { fg = "#d4d4d4" },
  Keyword = { fg = "#569cd6" },
  Exception = { fg = "#c586c0" },
  PreProc = { fg = "#c586c0" },
  Include = { fg = "#c586c0" },
  Define = { fg = "#c586c0" },
  Macro = { fg = "#dcdcaa" },
  PreCondit = { fg = "#c586c0" },
  Type = { fg = "#4ec9b0" },
  StorageClass = { fg = "#569cd6" },
  Structure = { fg = "#4ec9b0" },
  Typedef = { fg = "#4ec9b0" },
  Special = { fg = "#d7ba7d" },
  SpecialChar = { fg = "#d7ba7d" },
  Tag = { fg = "#569cd6" },
  Delimiter = { fg = "#808080" },
  SpecialComment = { fg = "#6a9955", italic = true },
  Debug = { fg = "#d16969" },

  DiagnosticError = { fg = "#f44747" },
  DiagnosticWarn = { fg = "#d7ba7d" },
  DiagnosticInfo = { fg = "#75beff" },
  DiagnosticHint = { fg = "#4ec9b0" },
  DiagnosticUnderlineError = { fg = "#f44747", sp = "#f44747", undercurl = true },
  DiagnosticUnderlineWarn = { fg = "#d7ba7d", sp = "#d7ba7d", undercurl = true },
  DiagnosticUnderlineInfo = { fg = "#75beff", sp = "#75beff", undercurl = true },
  DiagnosticUnderlineHint = { fg = "#4ec9b0", sp = "#4ec9b0", undercurl = true },

  ["@variable"] = { fg = "#e8e8e8" },
  ["@variable.parameter"] = { fg = "#9cdcfe" },
  ["@variable.member"] = { fg = "#9cdcfe" },
  ["@constant"] = { fg = "#b5cea8" },
  ["@string"] = { fg = "#ce9178" },
  ["@number"] = { fg = "#b5cea8" },
  ["@boolean"] = { fg = "#569cd6" },
  ["@function"] = { fg = "#dcdcaa" },
  ["@function.method"] = { fg = "#dcdcaa" },
  ["@constructor"] = { fg = "#4ec9b0" },
  ["@keyword"] = { fg = "#569cd6" },
  ["@keyword.function"] = { fg = "#569cd6" },
  ["@keyword.return"] = { fg = "#c586c0" },
  ["@type"] = { fg = "#4ec9b0" },
  ["@type.builtin"] = { fg = "#4ec9b0" },
  ["@property"] = { fg = "#9cdcfe" },
  ["@field"] = { fg = "#9cdcfe" },
  ["@module"] = { fg = "#4ec9b0" },
  ["@operator"] = { fg = "#d4d4d4" },
  ["@punctuation.delimiter"] = { fg = "#808080" },
  ["@punctuation.bracket"] = { fg = "#808080" },
  ["@punctuation.special"] = { fg = "#d7ba7d" },
  ["@escape"] = { fg = "#d7ba7d" },
  ["@embedded"] = { fg = "#e8e8e8" },
  ["@comment"] = { fg = "#6a9955", italic = true },

  ["@variable.nix"] = { fg = "#9cdcfe" },
  ["@variable.builtin.nix"] = { fg = "#b5cea8" },
  ["@function.nix"] = { fg = "#dcdcaa" },
  ["@function.builtin.nix"] = { fg = "#dcdcaa", italic = true },
  ["@property.nix"] = { fg = "#9cdcfe" },
  ["@escape.nix"] = { fg = "#d7ba7d" },
  ["@embedded.nix"] = { fg = "#e8e8e8" },

  ["@lsp.type.namespace"] = { fg = "#4ec9b0" },
  ["@lsp.type.type"] = { fg = "#4ec9b0" },
  ["@lsp.type.class"] = { fg = "#4ec9b0" },
  ["@lsp.type.enum"] = { fg = "#4ec9b0" },
  ["@lsp.type.interface"] = { fg = "#4ec9b0" },
  ["@lsp.type.struct"] = { fg = "#4ec9b0" },
  ["@lsp.type.parameter"] = { fg = "#9cdcfe" },
  ["@lsp.type.variable"] = { fg = "#e8e8e8" },
  ["@lsp.type.property"] = { fg = "#9cdcfe" },
  ["@lsp.type.enumMember"] = { fg = "#b5cea8" },
  ["@lsp.type.function"] = { fg = "#dcdcaa" },
  ["@lsp.type.method"] = { fg = "#dcdcaa" },
  ["@lsp.type.macro"] = { fg = "#dcdcaa" },
  ["@lsp.type.keyword"] = { fg = "#569cd6" },
  ["@lsp.type.comment"] = { fg = "#6a9955", italic = true },
  ["@lsp.type.string"] = { fg = "#ce9178" },
  ["@lsp.type.number"] = { fg = "#b5cea8" },
  ["@lsp.type.operator"] = { fg = "#d4d4d4" },
}

local function base16(p)
  if p.base16 then
    return p.base16
  end

  return {
    base00 = p.bg,
    base01 = p.line,
    base02 = p.surface,
    base03 = p.muted,
    base04 = p.muted,
    base05 = p.fg,
    base06 = p.fg,
    base07 = p.fg,
    base08 = p.error,
    base09 = p.number,
    base0A = p.type,
    base0B = p.string,
    base0C = p.constructor or p.special,
    base0D = p.func,
    base0E = p.keyword,
    base0F = p.special,
  }
end

local lsp_languages = {
  "rust",
  "python",
  "typescript",
  "typescriptreact",
  "tsx",
  "javascript",
  "javascriptreact",
  "go",
  "lua",
  "json",
  "jsonc",
  "toml",
  "yaml",
  "nix",
  "markdown",
}

local lsp_token_types = {
  "namespace",
  "type",
  "class",
  "enum",
  "interface",
  "struct",
  "typeParameter",
  "parameter",
  "variable",
  "property",
  "enumMember",
  "event",
  "function",
  "method",
  "macro",
  "keyword",
  "comment",
  "string",
  "number",
  "regexp",
  "operator",
  "decorator",
  "attribute",
  "attributeBracket",
  "builtinType",
  "escapeSequence",
  "formatSpecifier",
  "lifetime",
  "selfKeyword",
  "selfTypeKeyword",
  "typeAlias",
  "unresolvedReference",
}

local lsp_token_modifiers = {
  "declaration",
  "definition",
  "readonly",
  "static",
  "deprecated",
  "abstract",
  "async",
  "modification",
  "documentation",
  "defaultLibrary",
  "mutable",
  "unsafe",
  "attribute",
  "consuming",
  "controlFlow",
  "crateRoot",
  "injected",
  "library",
  "public",
  "reference",
  "trait",
  "callable",
}

local function merge_spec(a, b)
  local out = {}
  for key, value in pairs(a or {}) do
    out[key] = value
  end
  for key, value in pairs(b or {}) do
    out[key] = value
  end
  return out
end

local function lsp_type_spec(token, p, b)
  local map = {
    namespace = { fg = b.base0D },
    type = { fg = b.base0A },
    class = { fg = b.base0A },
    enum = { fg = b.base0A },
    interface = { fg = b.base0A },
    struct = { fg = b.base0A },
    typeParameter = { fg = b.base0A },
    parameter = { fg = b.base08 },
    variable = { fg = b.base05 },
    property = { fg = b.base08 },
    enumMember = { fg = b.base09 },
    event = { fg = b.base08 },
    ["function"] = { fg = b.base0D },
    method = { fg = b.base0D },
    macro = { fg = b.base08 },
    keyword = { fg = b.base0E },
    comment = { fg = p.comment, italic = true },
    string = { fg = b.base0B },
    number = { fg = b.base09 },
    regexp = { fg = b.base0C },
    operator = { fg = b.base05 },
    decorator = { fg = b.base0A },
    attribute = { fg = b.base0A },
    attributeBracket = { fg = b.base0F },
    builtinType = { fg = b.base0A },
    escapeSequence = { fg = b.base0C },
    formatSpecifier = { fg = b.base0C },
    lifetime = { fg = b.base0F },
    selfKeyword = { fg = b.base09 },
    selfTypeKeyword = { fg = b.base0A },
    typeAlias = { fg = b.base0A },
    unresolvedReference = { fg = p.error, undercurl = true },
  }
  return map[token] or { fg = b.base05 }
end

local function lsp_modifier_spec(modifier, p)
  local map = {
    declaration = { bold = true },
    definition = { bold = true },
    readonly = { italic = true },
    static = { italic = true },
    deprecated = { strikethrough = true },
    abstract = { italic = true },
    async = { italic = true },
    modification = { underline = true },
    documentation = { italic = true },
    defaultLibrary = { italic = true },
    mutable = { underline = true },
    unsafe = { fg = p.error },
    public = { bold = true },
    trait = { italic = true },
  }
  return map[modifier] or {}
end

local function themed_groups(p)
  local b = base16(p)
  return {
    Normal = { bg = p.bg, fg = p.fg },
    NormalNC = { bg = p.bg, fg = p.fg },
    NormalFloat = { bg = p.bg, fg = p.fg },
    FloatBorder = { bg = p.bg, fg = p.surface },
    EndOfBuffer = { bg = p.bg, fg = p.bg },
    SignColumn = { bg = p.bg },
    FoldColumn = { bg = p.bg, fg = p.muted },
    LineNr = { bg = p.bg, fg = p.muted },
    CursorLine = { bg = p.line },
    CursorLineNr = { bg = p.line, fg = p.fg, bold = true },
    Visual = { bg = p.surface },
    VisualNOS = { bg = p.surface },
    Search = { bg = p.warn, fg = p.bg },
    IncSearch = { bg = p.special, fg = p.bg },
    MatchParen = { bg = p.surface, fg = p.fg, bold = true },
    Pmenu = { bg = p.line, fg = p.fg },
    PmenuSel = { bg = p.surface, fg = p.fg },
    PmenuSbar = { bg = p.line },
    PmenuThumb = { bg = p.surface },
    StatusLine = { bg = p.bg, fg = p.fg },
    StatusLineNC = { bg = p.bg, fg = p.muted },
    WinSeparator = { bg = p.bg, fg = p.surface },
    VertSplit = { bg = p.bg, fg = p.surface },
    TabLine = { bg = p.bg, fg = p.muted },
    TabLineSel = { bg = p.surface, fg = p.fg },
    TabLineFill = { bg = p.bg },
    Cursor = { bg = p.fg, fg = p.bg },
    lCursor = { bg = p.fg, fg = p.bg },
    TermCursor = { bg = p.fg },

    Comment = { fg = p.comment, italic = true },
    Boolean = { fg = b.base09 },
    Character = { fg = b.base08 },
    Conditional = { fg = b.base0E },
    Constant = { fg = b.base09 },
    Define = { fg = b.base0E, sp = "none" },
    Delimiter = { fg = b.base0F },
    Float = { fg = b.base09 },
    Variable = { fg = b.base05 },
    Function = { fg = b.base0D },
    Identifier = { fg = b.base08, sp = "none" },
    Include = { fg = b.base0D },
    Keyword = { fg = b.base0E },
    Label = { fg = b.base0A },
    Number = { fg = b.base09 },
    Operator = { fg = b.base05, sp = "none" },
    PreProc = { fg = b.base0A },
    Repeat = { fg = b.base0A },
    Special = { fg = b.base0C },
    SpecialChar = { fg = b.base0F },
    Statement = { fg = b.base08 },
    StorageClass = { fg = b.base0A },
    String = { fg = b.base0B },
    Structure = { fg = b.base0E },
    Tag = { fg = b.base0A },
    Todo = { fg = b.base0A, bg = b.base01 },
    Type = { fg = b.base0A, sp = "none" },
    Typedef = { fg = b.base0A },
    SpecialComment = { fg = p.comment, italic = true },
    Debug = { fg = p.error },

    DiagnosticError = { fg = p.error },
    DiagnosticWarn = { fg = p.warn },
    DiagnosticInfo = { fg = p.info },
    DiagnosticHint = { fg = p.type },
    DiagnosticUnderlineError = { fg = p.error, sp = p.error, undercurl = true },
    DiagnosticUnderlineWarn = { fg = p.warn, sp = p.warn, undercurl = true },
    DiagnosticUnderlineInfo = { fg = p.info, sp = p.info, undercurl = true },
    DiagnosticUnderlineHint = { fg = p.type, sp = p.type, undercurl = true },

    ["@variable"] = { fg = b.base05 },
    ["@variable.builtin"] = { fg = b.base09 },
    ["@variable.parameter"] = { fg = b.base08 },
    ["@variable.parameter.builtin"] = { fg = b.base09 },
    ["@variable.member"] = { fg = b.base08 },
    ["@variable.member.key"] = { fg = b.base08 },
    ["@parameter"] = { fg = b.base08 },
    ["@field"] = { fg = b.base08 },
    ["@module"] = { fg = b.base08 },
    ["@module.builtin"] = { fg = b.base08 },
    ["@namespace"] = { fg = b.base08 },
    ["@constant"] = { fg = b.base09 },
    ["@constant.builtin"] = { fg = b.base09 },
    ["@constant.macro"] = { fg = b.base08 },
    ["@string"] = { fg = b.base0B },
    ["@string.documentation"] = { fg = b.base0B },
    ["@string.regex"] = { fg = b.base0C },
    ["@string.escape"] = { fg = b.base0C },
    ["@string.special"] = { fg = b.base0C },
    ["@string.special.symbol"] = { fg = b.base0C },
    ["@string.special.url"] = { fg = b.base09, underline = true },
    ["@string.special.path"] = { fg = b.base0B },
    ["@string.special.uri"] = { fg = b.base09 },
    ["@character"] = { fg = b.base08 },
    ["@character.special"] = { fg = b.base0F },
    ["@number"] = { fg = b.base09 },
    ["@number.float"] = { fg = b.base09 },
    ["@boolean"] = { fg = b.base09 },
    ["@annotation"] = { fg = b.base0F },
    ["@attribute"] = { fg = b.base0A },
    ["@error"] = { fg = b.base08 },
    ["@keyword.exception"] = { fg = b.base08 },
    ["@keyword"] = { fg = b.base0E },
    ["@keyword.coroutine"] = { fg = b.base0E },
    ["@keyword.function"] = { fg = b.base0E },
    ["@keyword.modifier"] = { fg = b.base0E },
    ["@keyword.return"] = { fg = b.base0E },
    ["@keyword.operator"] = { fg = b.base0E },
    ["@keyword.import"] = { link = "Include" },
    ["@keyword.conditional"] = { fg = b.base0E },
    ["@keyword.conditional.ternary"] = { fg = b.base0E },
    ["@keyword.repeat"] = { fg = b.base0A },
    ["@keyword.storage"] = { fg = b.base0A },
    ["@keyword.type"] = { fg = b.base0A },
    ["@keyword.debug"] = { fg = b.base08 },
    ["@keyword.directive.define"] = { fg = b.base0E },
    ["@keyword.directive"] = { fg = b.base0A },
    ["@function"] = { fg = b.base0D },
    ["@function.builtin"] = { fg = b.base0D },
    ["@function.macro"] = { fg = b.base08 },
    ["@function.call"] = { fg = b.base0D },
    ["@function.method"] = { fg = b.base0D },
    ["@function.method.call"] = { fg = b.base0D },
    ["@method"] = { fg = b.base0D },
    ["@method.call"] = { fg = b.base0D },
    ["@constructor"] = { fg = b.base0C },
    ["@operator"] = { fg = b.base05 },
    ["@reference"] = { fg = b.base05 },
    ["@punctuation.bracket"] = { fg = b.base0F },
    ["@punctuation.delimiter"] = { fg = b.base0F },
    ["@punctuation.special"] = { fg = b.base0F },
    ["@escape"] = { fg = b.base0C },
    ["@embedded"] = { fg = b.base05 },
    ["@symbol"] = { fg = b.base0B },
    ["@tag"] = { fg = b.base0A },
    ["@tag.attribute"] = { fg = b.base08 },
    ["@tag.delimiter"] = { fg = b.base0F },
    ["@type"] = { fg = b.base0A },
    ["@type.builtin"] = { fg = b.base0A },
    ["@type.definition"] = { fg = b.base0A },
    ["@type.qualifier"] = { fg = b.base0E },
    ["@definition"] = { sp = b.base04, underline = true },
    ["@scope"] = { bold = true },
    ["@property"] = { fg = b.base08 },
    ["@field"] = { fg = b.base08 },
    ["@markup.heading"] = { fg = b.base0D, bold = true },
    ["@markup.heading.1"] = { fg = b.base0D, bold = true },
    ["@markup.heading.2"] = { fg = b.base0E, bold = true },
    ["@markup.heading.3"] = { fg = b.base0A, bold = true },
    ["@markup.raw"] = { fg = b.base09 },
    ["@markup.link"] = { fg = b.base08 },
    ["@markup.link.url"] = { fg = b.base09, underline = true },
    ["@markup.link.label"] = { fg = b.base0C },
    ["@markup.list"] = { fg = b.base08 },
    ["@markup.list.checked"] = { fg = b.base0B },
    ["@markup.list.unchecked"] = { fg = b.base08 },
    ["@markup.strong"] = { bold = true },
    ["@markup.underline"] = { underline = true },
    ["@markup.italic"] = { italic = true },
    ["@markup.strikethrough"] = { strikethrough = true },
    ["@text"] = { fg = b.base05 },
    ["@text.emphasis"] = { fg = b.base09 },
    ["@text.strike"] = { fg = b.base0F, strikethrough = true },
    ["@text.strong"] = { bold = true },
    ["@text.underline"] = { underline = true },
    ["@text.literal"] = { fg = b.base09 },
    ["@text.uri"] = { fg = b.base09, underline = true },
    ["@text.reference"] = { fg = b.base08 },
    ["@text.title"] = { fg = b.base0D, bold = true },
    ["@text.todo"] = { fg = p.muted, bg = p.fg },
    ["@text.note"] = { fg = p.bg, bg = p.info },
    ["@text.warning"] = { fg = p.bg, bg = b.base09 },
    ["@text.danger"] = { fg = p.bg, bg = p.error },
    ["@comment"] = { fg = p.comment, italic = true },
    ["@comment.documentation"] = { fg = p.comment, italic = true },
    ["@comment.todo"] = { fg = p.muted, bg = p.fg },
    ["@comment.warning"] = { fg = p.bg, bg = b.base09 },
    ["@comment.note"] = { fg = p.bg, bg = p.info },
    ["@comment.danger"] = { fg = p.bg, bg = p.error },
    ["@diff.plus"] = { fg = p.string },
    ["@diff.minus"] = { fg = p.error },
    ["@diff.delta"] = { fg = p.muted },

    -- The Nix grammar captures attr names as both property and variable.
    -- Give Nix variables a real syntax color so flakes do not collapse
    -- back to plain foreground when the broader variable capture wins.
    ["@variable.nix"] = { fg = b.base08 },
    ["@variable.builtin.nix"] = { fg = b.base09 },
    ["@function.nix"] = { fg = b.base0D },
    ["@function.builtin.nix"] = { fg = b.base0D, italic = true },
    ["@property.nix"] = { fg = b.base08 },
    ["@escape.nix"] = { fg = b.base0C },
    ["@embedded.nix"] = { fg = b.base05 },
    ["@string.special.path.nix"] = { fg = b.base0B },
    ["@string.special.uri.nix"] = { fg = b.base09 },

    ["@lsp.type.namespace"] = { fg = b.base0D },
    ["@lsp.type.type"] = { fg = b.base0A },
    ["@lsp.type.class"] = { fg = b.base0A },
    ["@lsp.type.enum"] = { fg = b.base0A },
    ["@lsp.type.interface"] = { fg = b.base0A },
    ["@lsp.type.struct"] = { fg = b.base0A },
    ["@lsp.type.parameter"] = { fg = b.base08 },
    ["@lsp.type.variable"] = { fg = b.base05 },
    ["@lsp.type.property"] = { fg = b.base08 },
    ["@lsp.type.enumMember"] = { fg = b.base09 },
    ["@lsp.type.function"] = { fg = b.base0D },
    ["@lsp.type.method"] = { fg = b.base0D },
    ["@lsp.type.macro"] = { fg = b.base08 },
    ["@lsp.type.keyword"] = { fg = b.base0E },
    ["@lsp.type.comment"] = { fg = p.comment, italic = true },
    ["@lsp.type.string"] = { fg = b.base0B },
    ["@lsp.type.number"] = { fg = b.base09 },
    ["@lsp.type.operator"] = { fg = b.base05 },
    ["@lsp.type.attribute"] = { fg = b.base0A },
    ["@lsp.type.attributeBracket"] = { fg = b.base0F },
    ["@lsp.type.builtinType"] = { fg = b.base0A },
    ["@lsp.type.escapeSequence"] = { fg = b.base0C },
    ["@lsp.type.formatSpecifier"] = { fg = b.base0C },
    ["@lsp.type.lifetime"] = { fg = b.base0F },
    ["@lsp.type.selfKeyword"] = { fg = b.base09 },
    ["@lsp.type.selfTypeKeyword"] = { fg = b.base0A },
    ["@lsp.type.typeAlias"] = { fg = b.base0A },
    ["@lsp.type.unresolvedReference"] = { fg = p.error, undercurl = true },
    ["@lsp.mod.declaration"] = { bold = true },
    ["@lsp.mod.mutable"] = { underline = true },
    ["@lsp.mod.documentation"] = { italic = true },
    ["@lsp.mod.readonly"] = { italic = true },
    ["@lsp.mod.static"] = { italic = true },
    ["@lsp.mod.unsafe"] = { fg = p.error },
    ["@lsp.typemod.namespace.declaration"] = { fg = b.base0D, bold = true },
    ["@lsp.typemod.type.declaration"] = { fg = b.base0A, bold = true },
    ["@lsp.typemod.class.declaration"] = { fg = b.base0A, bold = true },
    ["@lsp.typemod.enum.declaration"] = { fg = b.base0A, bold = true },
    ["@lsp.typemod.interface.declaration"] = { fg = b.base0A, bold = true },
    ["@lsp.typemod.struct.declaration"] = { fg = b.base0A, bold = true },
    ["@lsp.typemod.function.declaration"] = { fg = b.base0D, bold = true },
    ["@lsp.typemod.method.declaration"] = { fg = b.base0D, bold = true },
    ["@lsp.typemod.variable.declaration"] = { fg = b.base05, bold = true },
    ["@lsp.typemod.parameter.declaration"] = { fg = b.base08, bold = true },
    ["@lsp.typemod.property.declaration"] = { fg = b.base08, bold = true },
    ["@lsp.typemod.enumMember.declaration"] = { fg = b.base09, bold = true },
    ["@lsp.typemod.variable.mutable"] = { fg = b.base05, underline = true },
    ["@lsp.typemod.parameter.mutable"] = { fg = b.base08, underline = true },
    ["@lsp.typemod.property.mutable"] = { fg = b.base08, underline = true },
    ["@lsp.typemod.variable.readonly"] = { fg = b.base09, italic = true },
    ["@lsp.typemod.property.readonly"] = { fg = b.base09, italic = true },
    ["@lsp.typemod.function.defaultLibrary"] = { fg = b.base0D, italic = true },
    ["@lsp.typemod.method.defaultLibrary"] = { fg = b.base0D, italic = true },
    ["@lsp.typemod.type.defaultLibrary"] = { fg = b.base0A, italic = true },
    ["@lsp.typemod.struct.defaultLibrary"] = { fg = b.base0A, italic = true },
  }
end

local function apply_generated_lsp_groups(p)
  local b = base16(p)

  for _, token in ipairs(lsp_token_types) do
    local type_group = "@lsp.type." .. token
    local type_spec = lsp_type_spec(token, p, b)
    vim.api.nvim_set_hl(0, type_group, type_spec)

    for _, lang in ipairs(lsp_languages) do
      vim.api.nvim_set_hl(0, type_group .. "." .. lang, { link = type_group })
    end
  end

  for _, modifier in ipairs(lsp_token_modifiers) do
    local mod_group = "@lsp.mod." .. modifier
    vim.api.nvim_set_hl(0, mod_group, lsp_modifier_spec(modifier, p))

    for _, lang in ipairs(lsp_languages) do
      vim.api.nvim_set_hl(0, mod_group .. "." .. lang, { link = mod_group })
    end
  end

  for _, token in ipairs(lsp_token_types) do
    local type_spec = lsp_type_spec(token, p, b)
    for _, modifier in ipairs(lsp_token_modifiers) do
      local group = "@lsp.typemod." .. token .. "." .. modifier
      vim.api.nvim_set_hl(0, group, merge_spec(type_spec, lsp_modifier_spec(modifier, p)))
      for _, lang in ipairs(lsp_languages) do
        vim.api.nvim_set_hl(0, group .. "." .. lang, { link = group })
      end
    end
  end
end

function M.apply(name)
  if name and palettes[name] then
    current_theme = name
  end
  local theme = palettes[current_theme]
  local base = theme and themed_groups(theme) or base_groups

  for name, spec in pairs(base) do
    vim.api.nvim_set_hl(0, name, spec)
  end

  if theme then
    apply_generated_lsp_groups(theme)
  end

  if not theme then
    for name, spec in pairs(syntax_groups) do
      vim.api.nvim_set_hl(0, name, spec)
    end
  end

  -- Force the external line-grid to republish every visible cell before
  -- this RPC returns. Neoism stores resolved StyleIds in its GPU grid; an
  -- hl_attr_define by itself cannot recolor cells that already reference
  -- the previous definition. This therefore has to be synchronous with
  -- the theme command, not a scheduled callback that may run after the
  -- Rust side has already consumed its one-shot full-damage marker.
  if vim.api.nvim__redraw then
    vim.api.nvim__redraw({ valid = false, flush = true })
  else
    vim.cmd("redraw!")
  end
end

function M.palette()
  return palettes[current_theme]
end

function M.setup()
  M.apply()
  vim.api.nvim_create_autocmd("ColorScheme", {
    callback = function()
      pcall(M.apply)
    end,
  })
end

return M

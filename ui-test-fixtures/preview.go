package fixture

import "strings"

type PatchState string

const (
	StateAdded   PatchState = "added"
	StateChanged PatchState = "changed"
	StateRemoved PatchState = "removed"
)

type PatchRow struct {
	Line    int
	State   PatchState
	Content string
}

func Label(row PatchRow) string {
	return strings.ToUpper(string(row.State)) + ": " + row.Content
}
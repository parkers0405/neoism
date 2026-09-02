---
name: "Agent sidebar pagination spinner and scroll anchor"
description: "Pagination spinner visible and append no longer jumps top"
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-29"
updated: "2026-08-29"
---

Follow-up to session pagination: desktop pagination spinner initially existed but was invisible because its +3/+4 orbit layers were below the badge at +5; spinner now accepts explicit render order and pagination uses badge +5, orbit +6/+7. Loading-more also jumped to top because renderer's old 8-second periodic first-page refresh could race a cursor request; the late first-page result replaced rows and reset scroll. Automatic refresh now runs only for Initial/Error, never Ready; Ready advances by cursor and directory changes explicitly invalidate. Starting pagination stamps refresh time. Regression asserts Ready does not auto-refresh and continuation append preserves scroll_top.

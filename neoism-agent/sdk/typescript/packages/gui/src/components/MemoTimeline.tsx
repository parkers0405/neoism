/** Compatibility entry point. Timeline itself is memoized so ordinary imports
 * also avoid whole-history work on unrelated composer/picker updates. */
export { Timeline as MemoTimeline } from './Timeline';

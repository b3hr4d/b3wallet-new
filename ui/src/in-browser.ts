import { useSyncExternalStore } from "react"

const never = () => () => {}

/**
 * False on the server and in the render that hydrates the prerendered page,
 * true from the next render on: what needs the page's address, the network
 * or the user renders only then, so hydration matches the prerendered HTML.
 */
export function useInBrowser(): boolean {
  return useSyncExternalStore(
    never,
    () => true,
    () => false,
  )
}

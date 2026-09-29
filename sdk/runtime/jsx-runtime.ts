// JSX runtime for Pocket UI. `pocket ts` compiles JSX with the automatic runtime and
// `pocket` as the import source, so `<box/>` becomes `jsx("box", {...})` from this module.
import { createElement, Fragment } from "./ui";
import type { VNode, VProps, ElementType } from "./ui";

export { Fragment };
export type { VNode };

export function jsx(type: ElementType, props: VProps & { children?: unknown }, key?: string | number): VNode {
    return createElement(type, { ...props, key: key ?? props.key }, ...normalize(props.children));
}

export function jsxs(type: ElementType, props: VProps & { children?: unknown }, key?: string | number): VNode {
    return jsx(type, props, key);
}

export const jsxDEV = jsx;

function normalize(children: unknown): unknown[] {
    if (children === undefined) return [];
    return Array.isArray(children) ? children : [children];
}

declare global {
    namespace JSX {
        type Element = VNode;
        interface ElementChildrenAttribute { children: unknown }
        /** Every element and component takes a `key`: the reconciler matches children by it across renders. */
        interface IntrinsicAttributes { key?: string | number }
        interface IntrinsicElements {
            box: import("./ui").BoxProps;
            text: import("./ui").TextProps;
            input: import("./ui").InputProps;
        }
    }
}

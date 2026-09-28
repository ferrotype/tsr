/** @jsx h */
export function h(type: any, props: any, ...children: any[]): h.JSX.Element { return null!; }
export namespace h {
  export namespace JSX {
    export interface Element { readonly pragma: true }
    export interface IntrinsicElements { p: { n: number } }
  }
}
export const a = <p n={1} />;
export const b = <><p n={1} /></>;

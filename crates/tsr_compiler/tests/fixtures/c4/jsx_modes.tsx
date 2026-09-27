import * as React from "react";
function Box(props: { title: string; children?: any }) { return <div>{props.title}</div>; }
export const a = <div id="x"><span /></div>;
export const b = <Box title="t"><span /></Box>;
export const c = <><Box title={1} /></>;
export const d: number = <span />;

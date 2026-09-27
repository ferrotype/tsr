type Head<S> = S extends `${infer H}${string}` ? H : never;
type Rest<S> = S extends `${string}${infer R}` ? R : never;
const head: Head<"😀abc"> = "😀";
const rest: Rest<"😀abc"> = "abc";
const wrongHead: Head<"😀abc"> = "x";
const wrongRest: Rest<"😀abc"> = "x";

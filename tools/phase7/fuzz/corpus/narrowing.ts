export type Choice = { kind: "number"; value: number } | { kind: "string"; value: string };
export function read(choice: Choice) {
    return choice.kind === "number" ? choice.value + 1 : choice.value;
}

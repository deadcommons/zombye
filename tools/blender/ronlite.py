"""A small RON reader for specs, plain Python (no Blender), shared by the Blender tools and
`tools/audio/zsound.py`."""


def read_ron(text):
    """A small RON reader for specs: structs (named or not) and maps become dicts, tuples and lists
    lists, `Some(x)` x, `None` None, other bare words strings."""
    i = 0
    n = len(text)

    def skip():
        nonlocal i
        while i < n:
            if text[i] in " \t\r\n,":
                i += 1
            elif text.startswith("//", i):
                i = text.find("\n", i)
                i = n if i < 0 else i
            elif text.startswith("/*", i):
                i = text.index("*/", i) + 2
            else:
                break

    def word():
        nonlocal i
        s = i
        while i < n and (text[i].isalnum() or text[i] == "_"):
            i += 1
        return text[s:i]

    def value():
        nonlocal i
        skip()
        c = text[i]
        if c == '"':
            j = i + 1
            out = []
            while text[j] != '"':
                if text[j] == "\\":
                    j += 1
                    out.append({"n": "\n", "t": "\t"}.get(text[j], text[j]))
                else:
                    out.append(text[j])
                j += 1
            i = j + 1
            return "".join(out)
        if c == "[":
            i += 1
            items = []
            while True:
                skip()
                if text[i] == "]":
                    i += 1
                    return items
                items.append(value())
        if c == "(":
            return group()
        if c == "{":
            i += 1
            d = {}
            while True:
                skip()
                if text[i] == "}":
                    i += 1
                    return d
                k = value()
                skip()
                assert text[i] == ":", f"RON: expected ':' at {i}"
                i += 1
                d[k] = value()
        if c in "-+.0123456789":
            s = i
            i += 1
            while i < n and (text[i] in "0123456789.eE_+-"):
                i += 1
            t = text[s:i].replace("_", "")
            return float(t) if any(ch in t for ch in ".eE") else int(t)
        w = word()
        if not w:
            raise ValueError(f"RON: unexpected {text[i]!r} at {i}")
        skip()
        if i < n and text[i] == "(":
            inner = group()
            if w == "Some":
                return inner[0] if isinstance(inner, list) and len(inner) == 1 else inner
            return inner
        return {"true": True, "false": False, "None": None}.get(w, w)

    def group():
        nonlocal i
        i += 1  # '('
        skip()
        save = i
        key = word()
        skip()
        if key and i < n and text[i] == ":":
            i = save
            d = {}
            while True:
                skip()
                if text[i] == ")":
                    i += 1
                    return d
                k = word()
                skip()
                assert text[i] == ":", f"RON: expected ':' at {i}"
                i += 1
                d[k] = value()
        i = save
        items = []
        while True:
            skip()
            if text[i] == ")":
                i += 1
                return items
            items.append(value())

    v = value()
    return v

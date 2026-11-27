# SPDX-License-Identifier: GPL-3.0-or-later
"""Capitalization of panel text.

Short status lines (the result headline, the Details lines) are Title Case;
full sentences (warnings, errors, hints, many from the native core) get a
capital first letter.
"""

# Joining words that stay lowercase inside a title.
MINOR = frozenset("a an and as at but by for from in into nor of on or per than the to via vs with".split())

# Pipeline stages by their native names, as the panel shows them.
STAGE_NAMES = {
    "validate": "Checking",
    "repair": "Cleanup",
    "features": "Sharp Features",
    "prep_remesh": "Prep Remesh",
    "field": "Cross Field",
    "frame": "Quad Shape",
    "trace": "Patch Layout",
    "layout": "Patch Layout",
    "quantize": "Quad Sizes",
    "patterns": "Patterns",
    "map": "Mapping",
    "smooth": "Smoothing",
    "attempts": "Layout Attempts",
    "output": "Output",
    "transfer": "Attributes",
}


def title(text):
    """Title Case: words capitalized except joining words ("the", "of", …),
    which are capitalized only first or after a colon. Words with digits
    ("p95", "2×") and one-letter units ("4.7 s") are left as they are."""
    words = text.split(" ")
    out = []
    start = True
    for word in words:
        out.append("-".join(_cap(part, start or i > 0) for i, part in enumerate(word.split("-"))))
        if word:
            start = word.endswith((":", ";"))
    return " ".join(out)


def _cap(token, force):
    first = next((i for i, ch in enumerate(token) if ch.isalpha()), None)
    if first is None or token[first].isupper() or any(ch.isdigit() for ch in token):
        return token
    core = token.strip("()[]{},;:.!?'\"").lower()
    if not force and (core in MINOR or len(core) == 1):
        return token
    return token[:first] + token[first].upper() + token[first + 1 :]


def sentence(text):
    """A capital first letter; the rest as written."""
    return text[:1].upper() + text[1:] if text else text


def stage_name(name):
    """The panel's name for a native stage name ("prep_remesh" → "Prep Remesh")."""
    return STAGE_NAMES.get(name, title(name.replace("_", " ")))


# What Detect Settings calls each kind of model, and why its suggestion suits it.
KINDS = {
    "smooth": ("Smooth Surface",
               "No crease lines to speak of, so Sharp Edges has nothing to follow. The settings stay as they are."),
    "hard_surface": ("Hard Surface",
                     "Nearly all its crease is in long or sharp lines. Every edge over the angle is kept."),
    "creased_sculpt": ("Sculpt With Creases",
                       "Long or sharp creases shape it, with scraps of surface detail beside them. "
                       "The scraps are ignored so they don't bend the edge flow."),
    "surface_detail": ("Surface Detail Only",
                       "Its creases are short scraps: fur, pores, scan noise or rounded rims. No setting "
                       "did better than another on models like this, so the settings stay as they are. "
                       "If the edge flow wanders, try Ignore Faint Creases at 9."),
    "unsure": ("Not Clear",
               "The crease lines don't say what kind of model this is. The settings stay as they are."),
}


def kind(name):
    """`(title, reason)` for a detected kind; an unknown name reads as not clear."""
    return KINDS.get(name, KINDS["unsure"])


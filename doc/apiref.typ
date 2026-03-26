#let col-pink = rgb("#e34967")
#let col-blue = rgb("#6A82FB")

#set page(
  margin: (y: 40pt, left: 40pt, right: 20pt),
  numbering: "1 /1",
  number-align: right,
)
#set text(14pt)

#show raw: it => box(
    fill: rgb("#F0F0F0"),
    radius: 3pt,
    inset: 3pt,
    baseline: 3pt,
    text(fill: col-blue, size: 10pt, smallcaps(it))
  )
#show link: it => underline(text(weight: "medium", fill: col-pink, size: 13pt, it))
#show ref: it => {
  let el = it.element
  link(el.location())[#it]
}

#show ref: it => underline(text(fill: col-pink, it))

#for endpoint in json("doc.json") {
  block(breakable: false)[
    #v(20pt)

    #text(fill: col-pink)[ *#endpoint.name* ]

    *#endpoint.method* #h(5pt) #box(stroke: col-blue, inset: 5pt, radius: 5pt, baseline: 5pt)[ #endpoint.url ]

    #endpoint.doc
  ]
}

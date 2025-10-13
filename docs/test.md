```mermaid
---
config:
  layout: fixed
---
flowchart TD
    N1(("1")) --> E3(("3"))
    E3 --> N2(("2<br>rdfs:Class"))
    E3 -- 5 --> N4(("4<br>rdfs:Label"))
     N1:::green
     E3:::green
     N2:::green
     N4:::green
    classDef green fill:#00ff55,stroke:#000,stroke-width:2px,color:#000

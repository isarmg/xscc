# GameTextInput InputConnection provenance

- Upstream: Android Open Source Project, frameworks/opt/gamesdk
- Revision: `4565303d567293ac474c37c2984741ffc0485c54`
  (GameActivity/GameTextInput 4.4.0 stable promotion)
- Source: https://android.googlesource.com/platform/frameworks/opt/gamesdk/+/4565303d567293ac474c37c2984741ffc0485c54/game-text-input/src/main/java/com/google/androidgamesdk/gametextinput/InputConnection.java
- Original source SHA256:
  `e9dbae371f1bda34e0d06525ff4f6f0f0e8ba718e910d53de686108c06ef5591`
- Matching Maven AAR: `androidx.games:games-activity:4.4.0`
- AAR SHA256: `3a8b0887ac55a0a6ab56021ced7353a250539eac811278b74bccd8a5b678d9c9`
- License: Apache-2.0; original source header retained.

Local patch: delete exactly 30 complete `Log.d(...)` statements, including
their argument expressions. No input/editing/lifecycle code is changed.
The two `Log.i` diagnostics about invalid cursor-request lengths remain.

R8 removed the debug output calls, but actual 4.4.0 DEX still executed
`String.format` on input text for the now-unused arguments. Source removal
avoids those temporary log strings without assuming that all `String.format`
calls in the application are side-effect-free.

The build filters only InputConnection.class and its two nested class files
from the SHA-pinned AAR. All other AAR contents are retained. This source
replaces those three classes; final DEX gates still verify the original
GameActivity class/member names and absence of sensitive log formats.

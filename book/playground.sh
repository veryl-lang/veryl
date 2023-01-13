#!/bin/sh
find ../html -name "*.html" | xargs sed -i "s/<pre><code class=\"language-veryl playground\">/<pre class=\"playground\"><code class=\"language-veryl\">/g"

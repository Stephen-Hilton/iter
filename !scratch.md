cd ~/dev/pdy-dev && ~/dev/iterapp/iter3/bin/iter_engine --config .iter/config.json

https://ivi88v0rwc.execute-api.us-west-2.amazonaws.com/
user:     gerald
password: gYDEij%L-VX5PApLRSr!


------------------------------------------------------------
------------------------------------------------------------
------------------------------------------------------------

Context to manage all MMORPG / multi-player networking components.   While this low-latency networking is challenging at the best of times, also recall the highly customized abilities of players means we'll need a concise way to transmit a higher-than-normal amount of data, at least the first time a new player enters the rendering radius, and again if they use experience points to update any abilities while in range.

The saving grace here: with many small player-built "instances" the grouping of players will actually be between 1 and 8 (or whater our assigned upper-bound is).  This means there will only be a few very-large lobby spaces (cities) where combat will rarely happen, although non-combat abilities will still need to function. 

The context here should include a design for MMORPG networking, and preferably one managed on a cloud vendor, like AWS or GCP or Azure, so costs scale with player base. 

this is a new version of the iter3 application

most of this doesn't change from iter3; 

# The same:
- the division of work between iter_data and iter_engine
- the web interface design 
- how the iter workitem works, with all the types and agents

# Fixes needed:
check for recent fixes being requested in iter3 by pdy-dev in the last 2 weeks, and (a) apply those to iter3 and (b) make sure we capture those ideas in iter4

# Different: 
- I want to re-introduce an idea that we had back in iter prime; the idea of a map of the architecture by assembling the *.iter.md files, and using the testgroups.iter.md to run tests (via schedule workitems) and generate new workitems when failures occur
- that said, I want to use a Graph DB for housing this data, so we can properly stitch together a real map of the application, beyond just the directory structure
- to support this, I want to change the data backend to ArangoDB, community edition (CE), which supports: Graph, document, key-value, vector and search in one engine
  - DynamoDB becomes either Arango document or key-value
  - C4 / tests / interfaces / usecases all becomes nodes (along with their metadata) and "children" relationships (today in the code.iter.md frontmatter) become edges
  - I want the engine to be somehow configurable to point to any iter_data instance, which itself can be a container with arango + iter_data rolled together (for easy deployment either locally with docker desktop or on a cloud, to a small VM / EC2)
- we'll also need iter_engine better able to send *.iter.md metadata to the iter_data server
  - maybe a validate() and/or sync() *.iter.md objects between the two iter objects (_engine local and _data remove/server)
  - to support this sync, we'll need every *.iter.md frontmatter to specify a unique ID
  - UUID is fine, just make sure every frontmatter has one
  - the validate() can also look for things like missing ids, and try to look it up via other markers in iter_data, and update if found, or assign new if not

# goal
At the end of this work, I should be able to point iter4 at it's own repo (`~/dev/iter/iter4/`), and have it interrogate the existing *.iter.md files and build a graph representation of the program structure in arango graph, and also migrate all DDB workitem (and other) records to arango key-value or document (whichever is more appropriate).


root/iter4dev


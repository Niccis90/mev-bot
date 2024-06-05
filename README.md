# mev-bot

This is an mev bot (searcher) written by Nicolas Natchev (@Niccis90) and Eelis Holmstén (@Eelishz).

The bot features:

   - A graph representation of prices
   - Multi-threaded depth first search of arbitrage opportunities
   - Asynchronous architecture using Tokio and Channels
   - Trade size optimization
   - Blacklisting of bad pools
   - Uniswap V2/V3 pools and flash loans
   - Trade simulation

## Goals of the project

The main goal of the project was to create a profitable MEV searcher using atomic arbitrage.

## Lessons learned

This project was much more complicated than either one of us initially anticipated.

Here is a non-exhaustive list of all the challenges we needed to tackle to complete this project:

   - Learning Rust’s async model
   - Multithreaded architecture
   - Database schema design and SQL (this idea was eventually abandoned in favor of in memory native data structures)
   - Learning Solidity (ethereum’s native language)
   - Dealing with blockchain security
   - Cloud hosting via AWS (Co-location with Bundle providers https://docs.flashbots.net/)
   - Running an ethereum node
   - Running a large project using git
   - Project management
   - Reading and understanding the spec of uniswap V2 and V3 and implementing parts of it ourselves

We chose rust as our primary programming language for this project as we were both familiar with it and it is a reasonably fast compiled language. Rust’s async features proved invaluable when dealing with long running multi-threaded processes and network requests. The main weakness of Rust is its complexity. Mixing async and synchronous code was especially challenging. Things like recursion and multithreading are, as far as we can tell, very hard to implement in an async event loop, and need to be synchronized. Channels were a useful tool in this endeavor.

Rust expressive syntax is a blessing and a curse. Our code styles are different and we would often find ourselves commenting on and refactoring each others’ code to a style which we viewed as “better”. This reinforced the need for common standards and style.

Learning the EVM (Ethereum virtual machine) was also very challenging. The EVM uses solidity as its primary programming language, an OOP Java style programming language. Setting up a testing environment, using a debugger, and deploying your code, usually simple things, are very hard on the blockchain.

Interop between Rust and the blockchain was also challenging since the EVM has much larger native types than regular languages.

The search algo itself was also a challenge. In the end, the solution was quite simple and the final version of the DFS (depth first search) algorithm is quite straight forward.


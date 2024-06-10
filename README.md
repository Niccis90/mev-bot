# MEV-Bot (Searcher)
Authors: Nicolas Natchev (@Niccis90) and Eelis Holmstén (@Eelishz)
# Overview
The MEV-Bot (Searcher) is an advanced automated trading bot designed to exploit arbitrage opportunities within decentralized finance (DeFi) ecosystems. Key features of the bot include:

- Graph representation of prices.
- Multi-threaded depth-first search for arbitrage opportunities.
- Asynchronous architecture leveraging Tokio and Channels.
- Trade size optimization.
- Blacklisting of non-performing pools.
- Compatibility with Uniswap V2/V3 pools and flash loans.
- Trade simulation.
- Project Objectives
The primary objective of this project was to develop a profitable MEV searcher utilizing atomic arbitrage strategies.

# Key Learnings
The project presented numerous complexities beyond initial expectations. Key challenges encountered include:

 - Learning Rust's Async Model: Mastering Rust’s asynchronous features was crucial for handling long-running, multi-threaded processes and network requests.
 - Multithreaded Architecture: Designing an efficient multi-threaded system to optimize performance.
 - Database Schema Design and SQL: Initially, a database approach was considered but later replaced by in-memory native data structures for efficiency.
 - Solidity Programming: Gaining proficiency in Solidity, the primary language for Ethereum smart contracts.
 - Blockchain Security: Implementing robust security measures within the blockchain environment.
 - Cloud Hosting via AWS: Co-locating with Bundle providers as per Flashbots documentation.
 - Running an Ethereum Node: Ensuring a reliable and efficient Ethereum node setup.
 - Large Project Management: Managing a large-scale project using Git and adhering to effective project management practices.
 - Uniswap V2/V3 Implementation: Reading, understanding, and partially implementing the specifications of Uniswap V2 and V3.
# Technology Choices
We selected Rust as our primary programming language due to its performance and familiarity. Rust’s asynchronous capabilities were particularly beneficial for managing long-running processes and network requests. However, Rust’s complexity, especially when mixing asynchronous and synchronous code, posed significant challenges.

We utilized Channels to synchronize recursion and multi-threading within an asynchronous event loop. The need for standardized code styles became evident, reinforcing the importance of common standards and style consistency.

# Ethereum Virtual Machine (EVM) Integration
Understanding and integrating with the Ethereum Virtual Machine (EVM) and Solidity posed substantial challenges. Setting up a testing environment, utilizing a debugger, and deploying code on the blockchain required significant effort. Interoperability between Rust and the blockchain was complex due to the EVM’s larger native types.

# Depth-First Search Algorithm
The development of the search algorithm was a key focus. Ultimately, the final version of the depth-first search (DFS) algorithm was straightforward and efficient, effectively identifying profitable arbitrage opportunities.
